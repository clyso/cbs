# Ceph Release Tool - patch commands
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
from collections.abc import Callable
from pathlib import Path

import click
from crtlib.apply import ApplyError, patches_apply_to_manifest
from crtlib.errors.manifest import MalformedManifestError, NoSuchManifestError
from crtlib.git_utils import GitError, git_prepare_remote, git_revparse
from crtlib.patch import (
    PatchError,
    PatchExistsError,
    patch_add,
    patch_import,
)

from cmds import Ctx, pass_ctx, perror, pinfo, psuccess, pwarn
from cmds import logger as parent_logger

logger = parent_logger.getChild("patchset")


def _cmd_validate_version_wrapper(
    allow_ces: bool,
) -> Callable[[click.Context, click.Parameter, str | None], str | None]:
    with_ces_re = r"^(ces-.*v|v){1}(\d+|\d+\.\d+|\d+\.\d+\.\d+(-.+)*)$"
    with_ceph_re = r"^v(\d+|\d+\.\d+|\d+\.\d+\.\d+)\+?$"

    def _cmd_validate_version(
        ctx: click.Context, param: click.Parameter, value: str | None
    ) -> str | None:
        if not value:
            return None

        match_re = with_ces_re if allow_ces else with_ceph_re
        if not re.match(match_re, value):
            raise click.BadParameter("malformed version", ctx, param)  # noqa: TRY003
        return value

    return _cmd_validate_version


def _cmd_validate_sha(
    ctx: click.Context, param: click.Parameter, value: list[str]
) -> list[str]:
    for entry in value:
        if not re.match(r"^[\da-f]{4}[\da-f]{0,36}$", entry):
            raise click.BadParameter("malformed SHA", ctx, param)  # noqa: TRY003
    return value


@click.group("patch", help="Handle patches.")
def cmd_patch() -> None:
    pass


@cmd_patch.command("import", help="Import a patch from a git repository.")
@click.option(
    "-c",
    "--ceph-git-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to patches git repository",
)
@click.option(
    "-t",
    "--target-version",
    "target_version",
    type=str,
    required=False,
    metavar="VERSION",
    help="Target CES version to import the patch(es) to.",
    callback=_cmd_validate_version_wrapper(True),
)
@click.option(
    "-s",
    "--src-version",
    "src_version",
    type=str,
    required=False,
    metavar="VERSION",
    help="Source Ceph version the patch(es) belong to.",
    callback=_cmd_validate_version_wrapper(False),
)
@click.argument(
    "patch_sha",
    metavar="SHA [SHA...]",
    type=str,
    required=True,
    nargs=-1,
    callback=_cmd_validate_sha,
)
@pass_ctx
def cmd_patch_import(
    _ctx: Ctx,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    target_version: str | None,
    src_version: str | None,
    patch_sha: list[str],
) -> None:
    if not ceph_repo_path.joinpath(".git").exists():
        perror(f"path at '{ceph_repo_path}' is not a git repository")
        sys.exit(errno.EINVAL)

    if not ceph_repo_path.joinpath("ceph.spec.in").exists():
        perror(f"path at '{ceph_repo_path}' is not a ceph repository")
        sys.exit(errno.EINVAL)

    if not patches_repo_path.joinpath(".git").exists():
        perror(f"path at '{patches_repo_path} is not a git repository")
        sys.exit(errno.EINVAL)

    try:
        shas = [git_revparse(ceph_repo_path, sha) for sha in patch_sha]
    except GitError as e:
        perror(f"unable to obtain sha: {e}")
        sys.exit(errno.EINVAL)

    if not shas:
        pwarn("no patches to import")
        sys.exit(errno.ENOENT)

    for sha in shas:
        try:
            patch_import(
                patches_repo_path.joinpath("ceph"),
                ceph_repo_path,
                sha,
                src_version=src_version,
                target_version=target_version,
            )
        except PatchExistsError:
            pinfo(f"patch sha '{sha}' already imported")
            return
        except PatchError as e:
            perror(f"unable to import patch sha '{sha}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        psuccess(f"imported patch sha '{sha}'")


@cmd_patch.command("add", help="Import a patch from a git repository.")
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to patches git repository",
)
@click.option(
    "--src-ceph-repo",
    "src_ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=False,
    help="Path to source ceph git repository",
)
@click.option(
    "--src-gh-repo",
    "src_gh_repo",
    required=False,
    type=str,
    metavar="OWNER/REPO",
    default="ceph/ceph",
    help="Specify the source remote repository to obtain the patch from",
    show_default=True,
)
@click.option(
    "-s",
    "--src-version",
    "src_version",
    type=str,
    required=False,
    metavar="VERSION",
    help="Source Ceph version the patch(es) belong to.",
    callback=_cmd_validate_version_wrapper(False),
)
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID for which information will be shown.",
)
@click.argument(
    "patch_sha",
    metavar="SHA [SHA...]",
    type=str,
    required=True,
    nargs=-1,
    callback=_cmd_validate_sha,
)
@pass_ctx
def cmd_patch_add(
    ctx: Ctx,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    src_ceph_repo_path: Path | None,
    src_gh_repo: str,
    src_version: str | None,
    manifest_uuid: uuid.UUID,
    patch_sha: list[str],
) -> None:
    if not ctx.github_token:
        perror("error: missing github token")
        sys.exit(errno.EINVAL)

    if not re.match(r"([\w\d_.-]+)/([\w\d_.-]+)", src_gh_repo):
        perror("error: malformed OWNER/REPO")
        sys.exit(errno.EINVAL)

    def _check_repo(repo_path: Path, what: str) -> None:
        if not repo_path.exists():
            perror(f"{what} repository does not exist at '{repo_path}'")
            sys.exit(errno.ENOENT)

        if not repo_path.joinpath(".git").exists():
            perror(f"provided path for {what} repository is not a git repository")
            sys.exit(errno.EINVAL)

    _check_repo(ceph_repo_path, "ceph")
    _check_repo(patches_repo_path, "patches")
    if src_ceph_repo_path:
        _check_repo(src_ceph_repo_path, "source ceph")
    else:
        src_ceph_repo_path = ceph_repo_path

    # update remote repo, maybe patches are not yet in the current repo state
    try:
        _ = git_prepare_remote(
            src_ceph_repo_path,
            f"github.com/{src_gh_repo}",
            src_gh_repo,
            ctx.github_token,
        )
    except Exception as e:
        perror(f"unable to update remote '{src_gh_repo}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        shas = [git_revparse(src_ceph_repo_path, sha) for sha in patch_sha]
    except GitError as e:
        perror(f"unable to obtain sha: {e}")
        sys.exit(errno.EINVAL)

    if not shas:
        pwarn("no patches to add")
        sys.exit(errno.ENOENT)

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

    # FIXME: check if patches already exist in patches repo

    # fetch patches from 'src_ceph_repo_path' to the patches repositor
    for sha in shas:
        logger.info(f"add patch sha '{sha}' to patches repository")
        try:
            patch_meta = patch_add(
                patches_repo_path, src_ceph_repo_path, sha, src_version
            )
        except PatchError as e:
            perror(
                f"unable to obtain patch sha '{sha}' from '{src_ceph_repo_path}': {e}"
            )
            sys.exit(errno.ENOTRECOVERABLE)
        except Exception as e:
            perror(f"unexpected error obtaining sha '{sha}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        logger.info(f"add patch sha '{sha}' to manifest")
        try:
            _, added, skipped = patches_apply_to_manifest(
                manifest,
                patch_meta,
                ceph_repo_path,
                patches_repo_path,
                ctx.github_token,
            )
        except (ApplyError, Exception) as e:
            perror(f"unable to apply patch sha '{sha}' to manifest: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        logger.debug(f"added: {added}")
        logger.debug(f"skipped: {skipped}")
        psuccess(f"successfully applied patch sha '{sha}' to manifest")

        if not manifest.add_patchset(patch_meta):
            perror("unexpected error adding patch to manifest !!")
            sys.exit(errno.ENOTRECOVERABLE)

        psuccess(f"patch sha '{sha}' added to manifest '{manifest_uuid}'")

    try:
        db.store_manifest(manifest)
    except Exception as e:
        perror(f"unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"successfully added patches to manifest '{manifest_uuid}'")
