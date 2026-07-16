# crt - apply manifest
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


import asyncio
import datetime
from datetime import datetime as dt
from pathlib import Path
from typing import override

from cbscommon.git.cmds import (
    get_git_user,
    git_am_abort,
    git_am_apply,
    git_branch_delete,
    git_checkout,
    git_prepare_remote,
    git_reset_state,
    git_update_submodules,
)
from cbscommon.git.exceptions import GitAMApplyError, GitError
from cbscommon.git.types import SHA

from crt.crtlib.logger import logger as parent_logger
from crt.crtlib.models.common import ManifestPatchEntry
from crt.crtlib.models.manifest import ReleaseManifest
from crt.crtlib.patch import PatchExistsError

logger = parent_logger.getChild("apply")


class ApplyError(Exception):
    msg: str | None

    def __init__(self, *, msg: str | None = None) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return "error applying manifest" + (f": {self.msg}" if self.msg else "")


class ApplyConflictError(ApplyError):
    sha: SHA
    conflict_files: list[str]

    def __init__(self, sha: SHA, files: list[str]) -> None:
        super().__init__(msg=f"{len(files)} file conflicts on sha '{sha}'")
        self.sha = sha
        self.conflict_files = files


def _prepare_repo(repo_path: Path):
    try:
        name, email = asyncio.run(get_git_user(repo_path))
        if not name:
            msg = "user's name not set for repository"
            logger.error(msg)
            raise ApplyError(msg=msg)
        if not email:
            msg = "user's email not set for repository"
            logger.error(msg)
            raise ApplyError(msg=msg)
    except GitError as e:
        msg = f"error obtaining repository's user's data: {e}"
        logger.error(msg)
        raise ApplyError(msg=msg) from None

    # propagate exceptions
    asyncio.run(git_reset_state(repo_path))
    asyncio.run(git_update_submodules(repo_path))


def apply_manifest(
    manifest: ReleaseManifest,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    target_branch: str,
    token: str,
    *,
    no_cleanup: bool = False,
    run_locally: bool = False,
) -> tuple[bool, list[ManifestPatchEntry], list[ManifestPatchEntry]]:
    logger.info(f"apply manifest '{manifest.release_uuid}' to branch '{target_branch}'")

    def _cleanup(*, abort_apply: bool = False) -> None:
        logger.debug(f"cleanup state, branch '{target_branch}'")
        if abort_apply:
            asyncio.run(git_am_abort(ceph_repo_path))

        asyncio.run(git_reset_state(ceph_repo_path))
        asyncio.run(git_branch_delete(ceph_repo_path, target_branch))

    def _apply_patches(
        patches: list[ManifestPatchEntry],
    ) -> tuple[list[ManifestPatchEntry], list[ManifestPatchEntry]]:
        logger.debug(f"apply {len(patches)} patches")

        skipped: list[ManifestPatchEntry] = []
        added: list[ManifestPatchEntry] = []

        for entry in patches:
            logger.debug(f"apply patch uuid '{entry.entry_uuid}'")

            patch_path = (
                patches_repo_path.joinpath("ceph")
                .joinpath("patches")
                .joinpath(f"{entry.entry_uuid}.patch")
            )
            if not patch_path.exists():
                raise ApplyError(msg=f"missing patch uuid '{entry.entry_uuid}'")

            try:
                asyncio.run(git_am_apply(ceph_repo_path, patch_path))
            except Exception as e:
                raise e from None

            added.append(entry)

        return (added, skipped)

    try:
        _prepare_repo(ceph_repo_path)
        repo_name = f"{manifest.base_ref_org}/{manifest.base_ref_repo}"
        if not run_locally:
            asyncio.run(
                git_prepare_remote(
                    ceph_repo_path, f"github.com/{repo_name}", repo_name, token
                )
            )
    except ApplyError as e:
        logger.error(e)
        raise e from None

    try:
        asyncio.run(
            git_checkout(
                ceph_repo_path,
                manifest.base_ref,
                to_branch=target_branch,
                clean=True,
                update_submodules=True,
            )
        )
    except ApplyError as e:
        msg = f"unable to apply manifest patchsets: {e}"
        logger.error(msg)
        if not no_cleanup:
            _cleanup()

        raise ApplyError(msg=msg) from e

    abort_am = True
    try:
        added, skipped = _apply_patches(manifest.patches)
        logger.debug("successfully applied patches to manifest")
    except (GitAMApplyError, Exception) as e:
        msg = f"failed applying manifest patches: {e}"
        logger.error(msg)
        raise ApplyError(msg=msg) from None
    else:
        abort_am = False
        logger.debug("git-am successful, don't abort on cleanup")
    finally:
        if not no_cleanup:
            _cleanup(abort_apply=abort_am)

    return (len(added) > 0, added, skipped)


def patches_apply_to_manifest(
    orig_manifest: ReleaseManifest,
    patch: ManifestPatchEntry,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    token: str,
    *,
    run_locally: bool = False,
) -> tuple[bool, list[ManifestPatchEntry], list[ManifestPatchEntry]]:
    manifest = orig_manifest.model_copy(deep=True)
    if not manifest.add_patches(patch):
        raise PatchExistsError(msg=f"uuid '{patch.entry_uuid}'")

    seq = dt.now(datetime.UTC).strftime("%Y%m%dT%H%M%S")
    target_branch = f"{manifest.name}-{manifest.release_git_uid}-{seq}"

    return apply_manifest(
        manifest,
        ceph_repo_path,
        patches_repo_path,
        target_branch,
        token,
        no_cleanup=False,
        run_locally=run_locally,
    )
