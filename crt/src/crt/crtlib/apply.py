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


import datetime
import logging
from datetime import datetime as dt
from pathlib import Path
from typing import override

import git
from cbscommon.git import (
    SHA,
    GitAMApplyError,
    git_am_abort,
    git_am_apply,
    git_checkout_from_local_ref,
    git_cleanup_repo,
    git_prepare_remote,
    git_update_submodules,
)  # Git types, exceptions and functions are now imported from cbscommon.git

from crt.crtlib.models.common import ManifestPatchEntry
from crt.crtlib.models.manifest import ReleaseManifest
from crt.crtlib.patch import PatchExistsError

logger = logging.getLogger(__name__)


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
    repo = git.Repo(repo_path)

    def _check_repo() -> None:
        logger.debug("check repo's config user and email")
        for what in ["name", "email"]:
            try:
                res = repo.git.execute(
                    ["git", "config", f"user.{what}"],
                    with_extended_output=False,
                    as_process=False,
                    stdout_as_string=True,
                )
            except Exception:
                msg = f"error obtaining repository's user's {what}"
                logger.error(msg)
                raise ApplyError(msg=msg) from None

            if not res:
                msg = f"user's {what} not set for repository"
                logger.error(msg)
                raise ApplyError(msg=msg)

    def _cleanup_repo() -> None:
        git_cleanup_repo(repo_path)
        repo.head.reference = repo.heads.main
        _ = repo.index.reset(index=True, working_tree=True)

    # propagate exceptions
    _check_repo()
    _cleanup_repo()
    git_update_submodules(repo_path)


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
    ceph_repo = git.Repo(ceph_repo_path)

    logger.info(f"apply manifest '{manifest.release_uuid}' to branch '{target_branch}'")

    def _cleanup(*, abort_apply: bool = False) -> None:
        logger.debug(f"cleanup state, branch '{target_branch}'")
        if abort_apply:
            git_am_abort(ceph_repo_path)

        git_cleanup_repo(ceph_repo_path)
        ceph_repo.head.reference = ceph_repo.heads.main
        ceph_repo.git.branch(["-D", target_branch])  # pyright: ignore[reportAny]

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
                git_am_apply(ceph_repo_path, patch_path)
            except Exception as e:
                raise e from None

            added.append(entry)

        return (added, skipped)

    try:
        _prepare_repo(ceph_repo_path)
        repo_name = f"{manifest.base_ref_org}/{manifest.base_ref_repo}"
        if not run_locally:
            git_prepare_remote(
                ceph_repo_path, f"github.com/{repo_name}", repo_name, token
            )
    except ApplyError as e:
        logger.error(e)
        raise e from None

    try:
        _branch = git_checkout_from_local_ref(
            ceph_repo_path, manifest.base_ref, target_branch
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
