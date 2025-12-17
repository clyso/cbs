# CBS Release Tool - release commands
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
from cbscore.versions.utils import parse_version
from git import GitError
from rich.padding import Padding
from rich.table import Table

from crt.cmds._common import CRTExitError, CRTProgress
from crt.crtlib.errors.manifest import NoSuchManifestError
from crt.crtlib.errors.release import NoSuchReleaseError
from crt.crtlib.git_utils import (
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
from crt.crtlib.manifest import load_manifest_by_name_or_uuid
from crt.crtlib.models.release import Release
from crt.crtlib.release import load_release, release_exists, store_release

from . import (
    console,
    perror,
    psuccess,
    pwarn,
    with_gh_token,
    with_patches_repo_path,
)
from . import (
    logger as parent_logger,
)

logger = parent_logger.getChild("release")

_ExitError = CRTExitError


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
        raise _ExitError(errno.ENOTRECOVERABLE) from e

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
        raise _ExitError(errno.ENOTRECOVERABLE) from e


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
        raise _ExitError(errno.ENOTRECOVERABLE) from e

    is_tag = False
    try:
        _ = git_fetch_ref(ceph_repo_path, src_ref, dst_branch, src_repo)
    except GitIsTagError:
        logger.debug(f"source ref '{src_ref}' is a tag, fetching as branch")
        is_tag = True
    except GitFetchHeadNotFoundError:
        perror(f"source ref '{src_ref}' not found in '{src_repo}'")
        raise _ExitError(errno.ENOENT) from None
    except GitError as e:
        perror(f"failed to fetch source ref '{src_ref}': {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from e

    if is_tag:
        try:
            git_branch_from(ceph_repo_path, src_ref, dst_branch)
        except GitError as e:
            perror(f"failed to create branch '{dst_branch}' from tag '{src_ref}': {e}")
            raise _ExitError(errno.ENOTRECOVERABLE) from e


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
    "--ref-rel-name",
    "from_ref_rel_name",
    type=str,
    required=False,
    metavar="RELEASE",
    help="Release name for the --ref argument (e.g., 'reef', 'squid').",
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
    from_ref_rel_name: str | None,
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
    elif from_ref and not from_ref_rel_name:
        perror("--ref requires --ref-rel-name to be set.")
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

    base_ref_rel_name: str
    base_ref: str
    base_ref_repo: str
    if from_manifest:
        try:
            manifest = load_manifest_by_name_or_uuid(patches_repo_path, from_manifest)
        except Exception as e:
            perror(f"Failed to load manifest '{from_manifest}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        base_ref_rel_name = manifest.base_release_name
        base_ref = f"release/{manifest.name}"
        base_ref_repo = manifest.dst_repo

    else:
        assert from_ref
        assert from_ref_rel_name
        m = re.match(r"(?:(?P<repo>.+)@)?(?P<ref>[\w\d_.-]+)", from_ref)
        if not m:
            perror("malformed '--ref' argument, expected format: [REPO@]REF")
            sys.exit(errno.EINVAL)

        base_ref_rel_name = from_ref_rel_name
        base_ref = cast(str, m.group("ref"))
        base_ref_repo = cast(str, m.group("repo") or "ceph/ceph")

    release_base_branch = f"release-base/{release_name}"
    release_base_tag = f"release-base-{release_name}"

    if release_exists(patches_repo_path, release_name):
        perror(f"release metadata for '{release_name}' already exists")
        sys.exit(errno.EEXIST)

    progress = CRTProgress(console)
    progress.start()

    progress.new_task("prepare repositories")
    try:
        _prepare_release_repo(
            ceph_repo_path,
            base_ref_repo,
            dst_repo,
            gh_token,
        )
    except _ExitError as e:
        progress.stop_error()
        sys.exit(e.code)
    except Exception as e:
        progress.stop_error()
        perror(f"failed to prepare release repositories: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()

    progress.new_task("prepare release branches")
    if git_get_remote_ref(ceph_repo_path, f"release/{release_name}", dst_repo):
        progress.stop_error()
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
    except _ExitError as e:
        progress.stop_error()
        sys.exit(e.code)
    except Exception as e:
        progress.stop_error()
        perror(f"failed to prepare release branches: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        _ = git_push(ceph_repo_path, release_base_branch, dst_repo)
    except GitError as e:
        progress.stop_error()
        perror(f"failed to push release branch '{release_base_branch}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        progress.stop_error()
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
        progress.stop_error()
        perror(f"failed to create and push tag '{release_base_tag}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()
    progress.stop()

    try:
        store_release(
            patches_repo_path,
            Release(
                name=release_name,
                base_release_name=base_ref_rel_name,
                base_release_ref=base_ref,
                base_repo=base_ref_repo,
                release_repo=dst_repo,
                release_base_branch=release_base_branch,
                release_base_tag=release_base_tag,
                release_branch=f"release/{release_name}",
            ),
        )
    except Exception as e:
        perror(f"failed to write release metadata for '{release_name}': {e}")
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
    "-d",
    "--dst-repo",
    "dst_repo",
    type=str,
    required=False,
    metavar="OWNER/REPO",
    default="clyso/ceph",
    help="Destination repository.",
    show_default=True,
)
@with_patches_repo_path
@with_gh_token
def cmd_release_list(
    gh_token: str, patches_repo_path: Path, ceph_repo_path: Path, dst_repo: str
) -> None:
    progress = CRTProgress(console)
    progress.start()

    progress.new_task("prepare remote")

    try:
        remote = git_prepare_remote(
            ceph_repo_path, f"github.com/{dst_repo}", dst_repo, gh_token
        )
    except GitError as e:
        perror(f"unable to prepare remote repository '{dst_repo}': {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()
    progress.stop()

    remote_releases: list[str] = []
    remote_base_releases: list[str] = []
    releases_meta: dict[str, Release | None] = {}

    for r in remote.refs:
        ref_name = r.name[len(dst_repo) + 1 :]
        m = re.match(r"(release|release-base)/((?:ces|ccs)-.+)", ref_name)
        if not m:
            continue

        if len(m.groups()) != 2:
            pwarn(f"unexpected release: {m.groups()}")
            continue

        rel_type = cast(str, m.group(1))
        rel_name = cast(str, m.group(2))

        if rel_type not in ["release", "release-base"]:
            perror(f"unknown release type '{rel_type}'")
            continue

        rel_meta: Release | None = None
        try:
            rel_meta = load_release(patches_repo_path, rel_name)
        except NoSuchReleaseError:
            pwarn(f"release '{rel_name}' missing metadata")
        except Exception as e:
            perror(f"failed to load release '{rel_name}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        releases_meta[rel_name] = rel_meta

        if rel_type == "release":
            remote_releases.append(rel_name)
        elif rel_type == "release-base":
            remote_base_releases.append(rel_name)

    not_released: list[str] = []
    for r in remote_base_releases:
        if r not in remote_releases:
            not_released.append(r)

    table = Table(show_header=True, show_lines=True, box=rich.box.HORIZONTALS)
    table.add_column("Name", justify="left", style="bold cyan", no_wrap=True)
    table.add_column("Base", justify="left", style="magenta", no_wrap=True)
    table.add_column("Status", justify="left", no_wrap=True)

    for r in remote_releases:
        rel = releases_meta.get(r)
        table.add_row(
            r, rel.base_release_name if rel else "n/a", "[green]released[/green]"
        )

    for r in not_released:
        rel = releases_meta.get(r)
        table.add_row(
            r, rel.base_release_name if rel else "n/a", "[yellow]not released[/yellow]"
        )

    console.print(Padding(table, (1, 0, 1, 0)))


@cmd_release.command("info", help="Show information about a release.")
@click.option(
    "-r",
    "--release",
    "release_name",
    type=str,
    required=False,
    help="Release name to show information for.",
)
@with_patches_repo_path
def cmd_release_info(patches_repo_path: Path, release_name: str | None) -> None:
    releases_path = patches_repo_path / "ceph" / "releases"
    if not releases_path.exists():
        pwarn(f"releases directory '{releases_path}' does not exist")
        sys.exit(errno.ENOENT)

    table = Table(show_header=False, show_lines=True, box=rich.box.HORIZONTALS)
    table.add_column("name", justify="left", style="bold cyan", no_wrap=True)
    table.add_column("info", justify="left", no_wrap=False)

    def _add_info_row(release: Release) -> None:
        info_table = Table(
            show_header=False,
            show_lines=False,
            box=None,
        )
        info_table.add_column("key", justify="right", style="magenta", no_wrap=True)
        info_table.add_column("value", justify="left", style="white", no_wrap=False)

        info_table.add_row("created", str(release.creation_date))
        info_table.add_row("base release", release.base_release_name)
        info_table.add_row("base ref", release.base_release_ref)
        info_table.add_row("base repo", release.base_repo)
        info_table.add_row("release repo", release.release_repo)
        info_table.add_row("release base branch", release.release_base_branch)
        info_table.add_row("release base tag", release.release_base_tag)
        info_table.add_row("release branch", release.release_branch)

        table.add_row(release.name, info_table)

    for r in releases_path.glob("*.json"):
        if release_name and r.stem != release_name:
            continue

        try:
            release = load_release(patches_repo_path, r.stem)
        except NoSuchReleaseError:
            perror(f"unexpected error loading release '{r.stem}'")
            sys.exit(errno.ENOTRECOVERABLE)
        except Exception as e:
            perror(f"failed to load release '{r.stem}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        _add_info_row(release)

    console.print(Padding(table, (1, 0, 1, 0)))


@cmd_release.command("finish", help="Finish a release.")
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
    "-m",
    "--manifest",
    "from_manifest",
    type=str,
    required=True,
    metavar="NAME|UUID",
    help="Manifest to finish the release from.",
)
@click.argument("release_name", type=str, required=True, metavar="NAME")
@with_patches_repo_path
@with_gh_token
def cmd_release_finish(
    gh_token: str,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    from_manifest: str,
    release_name: str,
) -> None:
    try:
        release = load_release(patches_repo_path, release_name)
    except NoSuchReleaseError:
        perror(f"release '{release_name}' does not exist")
        sys.exit(errno.ENOENT)
    except Exception as e:
        perror(f"failed to load release '{release_name}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if release.is_published:
        perror(f"release '{release_name}' is already published")
        sys.exit(errno.EEXIST)

    try:
        manifest = load_manifest_by_name_or_uuid(patches_repo_path, from_manifest)
    except NoSuchManifestError:
        perror(f"manifest '{from_manifest}' does not exist")
        sys.exit(errno.ENOENT)
    except Exception as e:
        perror(f"Failed to load manifest '{from_manifest}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not manifest.is_published:
        perror(f"manifest '{from_manifest}' is not published")
        sys.exit(errno.EINVAL)
    elif not manifest.dst_branch:
        perror(
            f"manifest '{from_manifest}' has no destination branch "
            + "-- most likely not published!"
        )
        sys.exit(errno.ENOTRECOVERABLE)

    progress = CRTProgress(console)
    progress.start()

    # 1. grab the manifest branch from its repository
    # 2. push the manifest branch to the release branch, in the release repository
    # 3. tag the release branch with the release name
    # 4. push the tag to the release repository
    # 5. mark the release as published and write it out

    progress.new_task("prepare repositories")
    logger.debug(
        f"prepare release repos, src: {manifest.dst_repo}, dst: {release.release_repo}"
    )
    try:
        _prepare_release_repo(
            ceph_repo_path, manifest.dst_repo, release.release_repo, gh_token
        )
    except _ExitError as e:
        progress.stop_error()
        sys.exit(e.code)
    except Exception as e:
        perror(f"failed to prepare release repositories: {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()
    progress.new_task("publish release")
    logger.debug(
        f"fetch manifest branch '{manifest.dst_branch}' to '{release.release_branch}'"
    )
    try:
        _ = git_fetch_ref(
            ceph_repo_path,
            manifest.dst_branch,
            release.release_branch,
            manifest.dst_repo,
        )
    except GitError as e:
        perror(f"failed to fetch manifest branch '{manifest.dst_branch}': {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    logger.debug(
        f"push release branch '{release.release_branch}' to '{release.release_repo}'"
    )
    try:
        _ = git_push(ceph_repo_path, release.release_branch, release.release_repo)
    except GitError as e:
        perror(f"failed to push release branch '{release.release_branch}': {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        perror(
            f"unexpected error pushing release branch '{release.release_branch}': {e}"
        )
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    logger.debug(
        f"tagging release branch '{release.release_branch}' with '{release.name}'"
    )
    try:
        git_tag(
            ceph_repo_path,
            release.name,
            release.release_branch,
            msg=f"Release '{release.name}'",
            push_to=release.release_repo,
        )
    except GitError as e:
        perror(f"failed to create and push tag '{release.name}': {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    release.is_published = True
    try:
        store_release(patches_repo_path, release)
    except Exception as e:
        perror(f"failed to write release metadata for '{release_name}': {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()
    progress.stop()

    psuccess(
        f"release '{release_name}' successfully published to '{release.release_repo}' "
        + f"branch '{release.release_branch}'"
    )
