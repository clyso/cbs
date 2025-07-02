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
from collections.abc import Callable
from pathlib import Path

import click
from crtlib.git_utils import GitError, git_revparse
from crtlib.patch import (
    PatchError,
    PatchExistsError,
    patch_import,
)

from . import Ctx, pass_ctx, perror, pinfo, psuccess, pwarn


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
