# Ceph Release Tool - patchset commands
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
import uuid
from pathlib import Path
from typing import cast

import click
from crtlib.apply import (
    ApplyConflictError,
    ApplyError,
    patches_apply_to_manifest,
    patchset_apply_to_manifest,
)
from crtlib.db.db import ReleasesDB
from crtlib.errors.manifest import (
    MalformedManifestError,
    NoSuchManifestError,
)
from crtlib.errors.patchset import (
    NoSuchPatchSetError,
    PatchSetError,
    PatchSetExistsError,
)
from crtlib.github import gh_get_pr
from crtlib.manifest import load_manifest, store_manifest
from crtlib.models.patchset import GitHubPullRequest
from crtlib.patchset import (
    patchset_fetch_gh_patches,
    patchset_get_gh,
    patchset_import_patches,
)
from crtlib.utils import print_patch_tree

from cmds import Ctx, pass_ctx, perror, pinfo, psuccess, pwarn, rprint
from cmds import logger as parent_logger

logger = parent_logger.getChild("patchset")


@click.group("patchset", help="Handle patch sets.")
def cmd_patchset() -> None:
    pass


@cmd_patchset.group("gh", help="Add a patch set from GitHub to a release.")
@click.option(
    "-c",
    "--ceph-git-path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository",
)
@pass_ctx
def cmd_patchset_gh(ctx: Ctx, ceph_git_path: Path) -> None:
    if not ceph_git_path.joinpath(".git").exists():
        perror(f"path at '{ceph_git_path}' is not a git repository")
        sys.exit(errno.EINVAL)

    if not ceph_git_path.joinpath("ceph.spec.in").exists():
        perror(f"path at '{ceph_git_path}' is not a ceph repository")
        sys.exit(errno.EINVAL)

    ctx.ceph_git_path = ceph_git_path


@cmd_patchset_gh.command("add", help="Add patch set from GitHub")
@click.argument(
    "pr_id",
    type=int,
    required=True,
    metavar="PR-ID",
)
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to which the patch set should be added.",
)
@click.option(
    "-r",
    "--repo",
    required=False,
    type=str,
    metavar="ORG/REPO",
    default="ceph/ceph",
    help="Specify the repository to obtain patch set from (default: ceph/ceph).",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to CES patches git repository.",
)
@pass_ctx
def cmd_patchset_gh_add(
    ctx: Ctx,
    pr_id: int,
    manifest_uuid: uuid.UUID,
    repo: str,
    patches_repo_path: Path,
) -> None:
    if not ctx.ceph_git_path or not ctx.github_token:
        perror("error: missing token or ceph git path")
        sys.exit(errno.EINVAL)

    m = re.match(r"([\w\d_.-]+)/([\w\d_.-]+)", repo)
    if not m:
        perror("error: malformed ORG/REPO")
        sys.exit(errno.EINVAL)

    org = cast(str, m.group(1))
    repo_name = cast(str, m.group(2))

    if not patches_repo_path.joinpath(".git").exists():
        perror(f"path at '{patches_repo_path} is not a git repository")
        sys.exit(errno.EINVAL)

    db = ctx.db

    try:
        manifest = db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not manifest.active_stage:
        perror(f"manifest uuid '{manifest_uuid}' has no active stage")
        pwarn(
            "please run '[bold bright_magenta]manifest stage new[/bold bright_magenta]'"
        )
        sys.exit(errno.ENOENT)

    patchset: GitHubPullRequest | None = None
    try:
        patchset = db.load_gh_pr(org, repo_name, pr_id)
    except NoSuchPatchSetError:
        pinfo("patch set not found, obtain from github")
    except PatchSetError as e:
        perror(f"unable to obtain patch set: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        perror(f"error: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not patchset:
        patchset = gh_get_pr(org, repo_name, pr_id, token=ctx.github_token)

        try:
            db.store_gh_patchset(patchset)
        except Exception as e:
            perror(
                f"unable to write patch set '{patchset.entry_uuid}' " + f"to disk: {e}"
            )
            sys.exit(errno.ENOTRECOVERABLE)

    pinfo("apply patches to manifest's repository")
    try:
        res, added, skipped = patchset_apply_to_manifest(
            db, manifest, patchset, ctx.ceph_git_path, ctx.github_token
        )
    except ApplyConflictError as e:
        perror(
            f"{len(e.conflict_files)} file conflicts found "
            + "applying patch set to manifest"
        )
        pinfo(f"on sha '{e.sha}':")
        for file in e.conflict_files:
            rprint(f"[red]\u203a {file}[/red]")

        sys.exit(errno.EAGAIN)

    except ApplyError as e:
        perror(f"unable to apply manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    except PatchSetExistsError:
        pwarn("patch set already in manifest, skip")
        return

    if added:
        print_patch_tree("patches added", added)

    if skipped:
        print_patch_tree("patches skipped", skipped)

    if not res:
        pinfo("no patches added, skip patch set")
        return

    if not manifest.add_patchset(patchset):
        perror("unexpected error adding patch set to manifest!!")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        patchset_import_patches(
            ctx.ceph_git_path, patches_repo_path.joinpath("ceph"), added, manifest.name
        )
    except Exception as e:
        perror(f"unable to import patch set patches: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        db.store_manifest(manifest)
    except Exception as e:
        perror(f"unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"pr id '{pr_id}' added to manifest '{manifest_uuid}'")


def _add_gh_patchset(  # pyright: ignore[reportUnusedFunction]
    db: ReleasesDB, pr_id: int, gh_repo_owner: str, gh_repo_name: str, token: str
) -> None:
    patchset: GitHubPullRequest | None = None
    try:
        patchset = db.load_gh_pr(gh_repo_owner, gh_repo_name, pr_id)
    except NoSuchPatchSetError:
        pinfo("patch set not found, obtain from github")
    except PatchSetError as e:
        perror(f"unable to obtain patch set: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        perror(f"error: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not patchset:
        patchset = gh_get_pr(gh_repo_owner, gh_repo_name, pr_id, token=token)

        try:
            db.store_gh_patchset(patchset)
        except Exception as e:
            perror(
                f"unable to write patch set '{patchset.entry_uuid}' " + f"to disk: {e}"
            )
            sys.exit(errno.ENOTRECOVERABLE)
    pass


@cmd_patchset.command("add", help="Add a new patch set to a release.")
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
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
    required=True,
    help="Path to the staging ceph git repository.",
)
@click.option(
    "--from-gh",
    type=str,
    required=False,
    metavar="PR_ID|URL",
    help="From a GitHub pull request.",
)
@click.option(
    "--from-gh-repo",
    type=str,
    required=False,
    metavar="OWNER/REPO",
    default="ceph/ceph",
    help="Specify GitHub repository to obtain patch set from",
    show_default=True,
)
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to which the patch set will be added.",
)
@pass_ctx
def cmd_patchset_add(
    ctx: Ctx,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    from_gh: str | None,
    from_gh_repo: str | None,
    manifest_uuid: uuid.UUID,
) -> None:
    if not ctx.github_token:
        perror("missing GitHub token")
        sys.exit(errno.EINVAL)

    def _check_repo(repo_path: Path, what: str) -> None:
        if not repo_path.exists():
            perror(f"{what} repository does not exist at '{repo_path}'")
            sys.exit(errno.ENOENT)

        if not repo_path.joinpath(".git").exists():
            perror(f"provided path for {what} repository is not a git repository")
            sys.exit(errno.EINVAL)

    def _get_gh_pr_data() -> tuple[int | None, str | None, str | None]:
        pr_id: int | None = None
        if from_gh:
            if m := re.match(r"^(\d+)$|^https://.*/pull/(\d+).*$", from_gh):
                pr_id = int(m.group(1))
            else:
                perror("malformed GitHub pull request ID or URL")
                sys.exit(errno.EINVAL)

        gh_owner: str | None = None
        gh_repo: str | None = None
        if from_gh_repo:
            if m := re.match(r"^([\w\d_.-]+)/([\w\d_.-]+)$", from_gh_repo):
                gh_owner = cast(str, m.group(1))
                gh_repo = cast(str, m.group(2))
            else:
                perror("malformed GitHub repository name")
                sys.exit(errno.EINVAL)

        if from_gh and not from_gh_repo:
            perror("missing GitHub repository to obtain patch set from")
            sys.exit(errno.EINVAL)

        return (pr_id, gh_owner, gh_repo)

    _check_repo(patches_repo_path, "patches")
    _check_repo(ceph_repo_path, "ceph")
    gh_pr_id, gh_repo_owner, gh_repo = _get_gh_pr_data()

    # db = ctx.db

    try:
        manifest = load_manifest(patches_repo_path, manifest_uuid)
        # manifest = db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not manifest.active_stage:
        perror(f"manifest uuid '{manifest_uuid}' has no active stage")
        pwarn(
            "please run '[bold bright_magenta]manifest stage new[/bold bright_magenta]'"
        )
        sys.exit(errno.ENOENT)

    if not gh_pr_id:
        # FIXME: for now, we don't deal with anything other than gh patch sets
        pwarn("not currently supported")
        return

    # FIXME: this must be properly checked once we support more than just gh prs
    assert gh_repo_owner
    assert gh_repo

    patchset: GitHubPullRequest | None = None
    try:
        patchset = patchset_get_gh(patches_repo_path, gh_repo_owner, gh_repo, gh_pr_id)
        pinfo("found patch set")
    except NoSuchPatchSetError:
        pinfo("patch set not found, obtain from github")
    except PatchSetError as e:
        perror(f"unable to obtain patch set: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        perror(f"error found: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not patchset:
        # obtain from github
        try:
            patchset = gh_get_pr(
                gh_repo_owner, gh_repo, gh_pr_id, token=ctx.github_token
            )
            patchset_fetch_gh_patches(
                ceph_repo_path, patches_repo_path, patchset, ctx.github_token
            )
        except PatchSetError as e:
            perror(f"unable to obtain patch set: {e}")
            sys.exit(errno.ENOTRECOVERABLE)
        except Exception as e:
            perror(f"unexpected error: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

    pinfo("apply patch set to manifest's repository")
    try:
        _, added, skipped = patches_apply_to_manifest(
            manifest, patchset, ceph_repo_path, patches_repo_path, ctx.github_token
        )
    except (ApplyError, Exception) as e:
        perror(f"unable to apply to manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    logger.debug(f"added: {added}")
    logger.debug(f"skipped: {skipped}")
    psuccess("successfully applied patch set to manifest")

    if not manifest.add_patchset(patchset):
        perror("unexpected error adding patch set to manifest !!")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        store_manifest(patches_repo_path, manifest)
        # db.store_manifest(manifest)
    except Exception as e:
        perror(f"unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"pr id '{gh_pr_id}' added to manifest '{manifest_uuid}'")
