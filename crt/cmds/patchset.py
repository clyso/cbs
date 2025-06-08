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
from crtlib.apply import ApplyError, apply_manifest
from crtlib.github import gh_get_pr
from crtlib.manifest import MalformedManifestError, NoSuchManifestError
from crtlib.patch import Patch
from crtlib.patchset import GitHubPullRequest, NoSuchPatchSetError, PatchSetError

from cmds import Ctx, pass_ctx
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
        click.echo(
            f"error: path at '{ceph_git_path}' is not a git repository", err=True
        )
        sys.exit(errno.EINVAL)

    if not ceph_git_path.joinpath("ceph.spec.in").exists():
        click.echo(
            f"error: path at '{ceph_git_path}' is not a ceph repository", err=True
        )
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
        click.echo("error: missing token or ceph git path", err=True)
        sys.exit(errno.EINVAL)

    m = re.match(r"([\w\d_.-]+)/([\w\d_.-]+)", repo)
    if not m:
        click.echo("error: malformed ORG/REPO", err=True)
        sys.exit(errno.EINVAL)

    org = cast(str, m.group(1))
    repo_name = cast(str, m.group(2))

    db = ctx.db

    try:
        manifest = db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        click.echo(f"error: unable to find manifest '{manifest_uuid}' in db", err=True)
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        click.echo(f"error: malformed manifest '{manifest_uuid}'", err=True)
        sys.exit(errno.EINVAL)
    except Exception as e:
        click.echo(f"error: unable to obtain manifest '{manifest_uuid}': {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    patchset: GitHubPullRequest | None = None
    try:
        patchset = db.load_gh_pr(org, repo_name, pr_id)
    except NoSuchPatchSetError:
        click.echo("patch set not found, obtain from github")
    except PatchSetError as e:
        click.echo(f"error: unable to obtain patch set: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        click.echo(f"error: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    if not patchset:
        patchset = gh_get_pr(org, repo_name, pr_id, token=ctx.github_token)
        click.echo(f"patchset:\n{patchset}")

        try:
            db.store_gh_patchset(patchset)
        except Exception as e:
            click.echo(
                f"error: unable to write patch set '{patchset.patchset_uuid}' "
                + f"to disk: {e}",
                err=True,
            )
            sys.exit(errno.ENOTRECOVERABLE)

    logger.debug(f"add patchset '{patchset.patchset_uuid}' to in-mem manifest")
    if not manifest.add_patchset(patchset):
        # FIXME: make this an output to console
        logger.info(f"patchset '{patchset.patchset_uuid}' already in manifest")
        return

    click.echo("apply patches to manifest's repository")
    try:
        res, added, skipped = apply_manifest(
            db, manifest, ctx.ceph_git_path, ctx.github_token
        )
    except ApplyError as e:
        click.echo(f"error: unable to apply manifest: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    def _print_patches(lst: list[Patch]) -> None:
        for patch in lst:
            print(f"> {patch.title} ({patch.sha})")

    if added:
        print("patches added:")
        _print_patches(added)

    if skipped:
        print("patches skipped:")
        _print_patches(skipped)

    if not res:
        print("no patches added, skip patch set")
        return

    try:
        db.store_manifest(manifest)
    except Exception as e:
        click.echo(f"error: unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
