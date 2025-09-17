# Ceph Release Tool - release commands
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.

import errno
import re
import sys
from pathlib import Path
from typing import cast

import click
import rich.box
from crtlib.git_utils import (
    GitFetchHeadNotFoundError,
    GitIsTagError,
    git_branch_from,
    git_cleanup_repo,
    git_fetch_ref,
    git_get_remote_ref,
    git_prepare_remote,
    git_push,
    git_reset_head,
    git_tag,
)
from crtlib.manifest import load_manifest_by_name_or_uuid
from crtlib.utils import parse_version
from git import GitError
from rich.padding import Padding
from rich.table import Table

from . import (
    console,
    perror,
    pwarn,
    with_gh_token,
    with_patches_repo_path,
)
from . import (
    logger as parent_logger,
)

logger = parent_logger.getChild("release")


def _prepare_release_repo(
    ceph_repo_path: Path,
    src_repo: str,
    dst_repo: str,
    token: str,
) -> None:
    try:
        git_cleanup_repo(ceph_repo_path)
        git_reset_head(ceph_repo_path, "main")
    except GitError as e:
        perror(f"failed to cleanup ceph repo at '{ceph_repo_path}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        _ = git_prepare_remote(
            ceph_repo_path, f"github.com/{src_repo}", src_repo, token
        )
        if src_repo != dst_repo:
            _ = git_prepare_remote(
                ceph_repo_path, f"github.com/{dst_repo}", dst_repo, token
            )
    except GitError as e:
        perror(f"failed to prepare git remotes: {e}")
        sys.exit(errno.ENOTRECOVERABLE)


def _prepare_release_branches(
    ceph_repo_path: Path,
    src_repo: str,
    src_ref: str,
    dst_repo: str,
    dst_branch: str,
) -> None:
    try:
        if git_get_remote_ref(ceph_repo_path, dst_branch, dst_repo):
            perror(f"destination branch '{dst_branch}' already exists in '{dst_repo}'")
            sys.exit(errno.EEXIST)
    except GitError as e:
        perror(f"failed to check for existing branch '{dst_branch}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    is_tag = False
    try:
        _ = git_fetch_ref(ceph_repo_path, src_ref, dst_branch, src_repo)
    except GitIsTagError:
        logger.debug(f"source ref '{src_ref}' is a tag, fetching as branch")
        is_tag = True
    except GitFetchHeadNotFoundError:
        perror(f"source ref '{src_ref}' not found in '{src_repo}'")
        sys.exit(errno.ENOENT)
    except GitError as e:
        perror(f"failed to fetch source ref '{src_ref}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if is_tag:
        try:
            git_branch_from(ceph_repo_path, src_ref, dst_branch)
        except GitError as e:
            perror(f"failed to create branch '{dst_branch}' from tag '{src_ref}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)


@click.group("release", help="Release operations.")
def cmd_release():
    pass


@cmd_release.command("start", help="Start a new release.")
@click.option(
    "--from",
    "from_manifest",
    type=str,
    required=False,
    metavar="NAME|UUID",
    help="Manifest to start from.",
)
@click.option(
    "--ref",
    "from_ref",
    type=str,
    required=False,
    metavar="[REPO@]REF",
    help="Reference to start from.",
)
@click.option(
    "-r",
    "--repo",
    "dst_repo",
    type=str,
    required=False,
    metavar="OWNER/REPO",
    default="clyso/ceph",
    help="Destination repository.",
    show_default=True,
)
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    envvar="CRT_CEPH_REPO_PATH",
    required=True,
    help="Path to the staging ceph git repository.",
)
@click.option(
    "--allow-dev",
    is_flag=True,
    default=False,
    required=False,
    help="Allow a development release, with suffixes.",
)
@click.argument("release_name", type=str, required=True, metavar="NAME")
@with_patches_repo_path
@with_gh_token
def cmd_release_start(
    gh_token: str,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    from_manifest: str | None,
    from_ref: str | None,
    dst_repo: str,
    allow_dev: bool,
    release_name: str,
) -> None:
    if from_manifest and from_ref:
        perror("Cannot use --from and --ref together.")
        sys.exit(errno.EINVAL)
    elif not from_manifest and not from_ref:
        perror("Either --from or --ref must be provided.")
        sys.exit(errno.EINVAL)

    # enforce strict naming criteria
    try:
        prefix, _, minor, patch, suffix = parse_version(release_name)
    except ValueError:
        perror(f"invalid release name '{release_name}'")
        sys.exit(errno.EINVAL)

    if not prefix or prefix not in ["ces", "ccs"]:
        perror(f"invalid release name prefix '{prefix}', expected 'ces' or 'ccs'")
        sys.exit(errno.EINVAL)

    if not minor or not patch:
        perror("malformed release name, missing minor or patch version")
        sys.exit(errno.EINVAL)

    if suffix and not allow_dev:
        perror(
            f"release name '{release_name}' contains suffix '{suffix}', "
            + "use '--allow-dev' to override"
        )
        sys.exit(errno.EINVAL)

    base_ref: str
    base_ref_repo: str
    if from_manifest:
        try:
            manifest = load_manifest_by_name_or_uuid(patches_repo_path, from_manifest)
        except Exception as e:
            perror(f"Failed to load manifest '{from_manifest}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        base_ref = f"release/{manifest.name}"
        base_ref_repo = manifest.dst_repo

    else:
        assert from_ref
        m = re.match(r"(?:(?P<repo>.+)@)?(?P<ref>[\w\d_.-]+)", from_ref)
        if not m:
            perror("malformed '--ref' argument, expected format: [REPO@]REF")
            sys.exit(errno.EINVAL)

        base_ref = cast(str, m.group("ref"))
        base_ref_repo = cast(str, m.group("repo") or "ceph/ceph")

    release_base_branch = f"release-base/{release_name}"
    release_base_tag = f"release-base-{release_name}"

    try:
        _prepare_release_repo(
            ceph_repo_path,
            base_ref_repo,
            dst_repo,
            gh_token,
        )
    except Exception as e:
        perror(f"failed to prepare release repositories: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if git_get_remote_ref(ceph_repo_path, f"release/{release_name}", dst_repo):
        perror(f"release '{release_name}' already marked released in '{dst_repo}'")
        sys.exit(errno.EEXIST)

    try:
        _prepare_release_branches(
            ceph_repo_path,
            base_ref_repo,
            base_ref,
            dst_repo,
            release_base_branch,
        )
    except Exception as e:
        perror(f"failed to prepare release branches: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        _ = git_push(ceph_repo_path, release_base_branch, dst_repo)
    except GitError as e:
        perror(f"failed to push release branch '{release_base_branch}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        perror(f"unexpected error pushing release branch '{release_base_branch}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        git_tag(
            ceph_repo_path,
            release_base_tag,
            release_base_branch,
            msg=f"Base release for {release_name}",
            push_to=dst_repo,
        )
    except GitError as e:
        perror(f"failed to create and push tag '{release_base_tag}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    summary_table = Table(show_header=False, show_lines=False, box=None)
    summary_table.add_column(justify="right", style="bold cyan", no_wrap=True)
    summary_table.add_column(justify="left", style="magenta", no_wrap=False)
    summary_table.add_row("Release Name", release_name)
    summary_table.add_row("Destination Repo", dst_repo)
    summary_table.add_row("Ceph Repo Path", str(ceph_repo_path))
    summary_table.add_row("From Manifest", from_manifest or "n/a")
    summary_table.add_row("From Base Reference", f"{base_ref} from {base_ref_repo}")
    summary_table.add_row("Release base branch", release_base_branch)
    summary_table.add_row("Release base tag", release_base_tag)

    console.print(Padding(summary_table, (1, 0, 1, 0)))


@cmd_release.command("list", help="List existing releases.")
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    envvar="CRT_CEPH_REPO_PATH",
    required=True,
    help="Path to the staging ceph git repository.",
)
@click.option(
    "-r",
    "--repo",
    "repo",
    type=str,
    required=False,
    metavar="OWNER/REPO",
    default="clyso/ceph",
    help="Destination repository.",
    show_default=True,
)
@with_gh_token
def cmd_release_list(gh_token: str, ceph_repo_path: Path, repo: str) -> None:
    try:
        remote = git_prepare_remote(
            ceph_repo_path, f"github.com/{repo}", repo, gh_token
        )
    except GitError as e:
        perror(f"unable to prepare remote repository '{repo}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    remote_releases: list[str] = []
    remote_base_releases: list[str] = []

    for r in remote.refs:
        ref_name = r.name[len(repo) + 1 :]
        m = re.match(r"(release|release-base)/((?:ces|ccs)-.+)", ref_name)
        if not m:
            continue

        if len(m.groups()) != 2:
            pwarn(f"unexpected release: {m.groups()}")
            continue

        rel_type = cast(str, m.group(1))
        rel_name = cast(str, m.group(2))

        if rel_type == "release":
            remote_releases.append(rel_name)
        elif rel_type == "release-base":
            remote_base_releases.append(rel_name)
        else:
            perror(f"unknown release type '{rel_type}'")
            continue

    not_released: list[str] = []
    for r in remote_base_releases:
        if r not in remote_releases:
            not_released.append(r)

    table = Table(show_header=False, show_lines=True, box=rich.box.HORIZONTALS)
    table.add_column("Release Name", justify="left", style="bold cyan", no_wrap=True)
    table.add_column("Status", justify="left", no_wrap=True)

    for r in remote_releases:
        table.add_row(r, "[green]released[/green]")

    for r in not_released:
        table.add_row(r, "[yellow]not released[/yellow]")

    console.print(Padding(table, (1, 0, 1, 0)))
