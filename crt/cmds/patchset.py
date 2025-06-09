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
from crtlib.apply import ApplyConflictError, ApplyError, patchset_apply_to_manifest
from crtlib.errors.manifest import MalformedManifestError, NoSuchManifestError
from crtlib.errors.patchset import (
    NoSuchPatchSetError,
    PatchSetError,
    PatchSetExistsError,
)
from crtlib.github import gh_get_pr
from crtlib.models.patchset import GitHubPullRequest
from crtlib.utils import print_patch_tree

from cmds import Ctx, pass_ctx, perror, pinfo, psuccess, pwarn, rprint
from cmds import logger as parent_logger

logger = parent_logger.getChild("patchset")


@click.group("patchset", help="Handle patch sets.")
def cmd_patchset() -> None:
    pass


@cmd_patchset.group("add", help="Add a patch set to a release.")
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
def cmd_patchset_add(ctx: Ctx, ceph_git_path: Path) -> None:
    if not ceph_git_path.joinpath(".git").exists():
        perror(f"path at '{ceph_git_path}' is not a git repository")
        sys.exit(errno.EINVAL)

    if not ceph_git_path.joinpath("ceph.spec.in").exists():
        perror(f"path at '{ceph_git_path}' is not a ceph repository")
        sys.exit(errno.EINVAL)

    ctx.ceph_git_path = ceph_git_path


@cmd_patchset_add.command("gh", help="Add patch set from GitHub")
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
@pass_ctx
def cmd_patchset_add_gh(
    ctx: Ctx, pr_id: int, manifest_uuid: uuid.UUID, repo: str
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

    db = ctx.db

    try:
        manifest = db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        perror(f"error: unable to find manifest '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"error: malformed manifest '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"error: unable to obtain manifest '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

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
        logger.debug(f"obtained patchset: {patchset}")

        try:
            db.store_gh_patchset(patchset)
        except Exception as e:
            perror(
                f"unable to write patch set '{patchset.patchset_uuid}' "
                + f"to disk: {e}"
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
        db.store_manifest(manifest)
    except Exception as e:
        perror(f"unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"pr id '{pr_id}' added to manifest '{manifest_uuid}'")
