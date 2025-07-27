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
from datetime import datetime as dt
from pathlib import Path
from typing import override

import git
from crtlib.git_utils import (
    SHA,
    GitAMApplyError,
    git_am_abort,
    git_am_apply,
    git_cleanup_repo,
    git_get_local_head,
    git_prepare_remote,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.common import ManifestPatchEntry
from crtlib.models.manifest import ReleaseManifest
from crtlib.patch import PatchExistsError

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


def _update_submodules(repo_path: Path) -> None:
    logger.debug("update submodules")
    repo = git.Repo(repo_path)
    try:
        repo.git.execute(  # pyright: ignore[reportCallIssue]
            ["git", "submodule", "update", "--init", "--recursive"],
            as_process=False,
            with_stdout=True,
        )
    except Exception as e:
        msg = f"unable to update repository's submodules: {e}"
        logger.error(msg)
        raise ApplyError(msg=msg) from None


def _checkout_ref(repo_path: Path, from_ref: str, branch_name: str) -> git.Head:
    logger.debug(f"checkout ref '{from_ref}' to '{branch_name}'")
    repo = git.Repo(repo_path)
    if head := git_get_local_head(repo_path, branch_name):
        logger.debug(f"branch '{branch_name}' already exists, simply checkout")
        repo.head.reference = head
        _ = repo.head.reset(index=True, working_tree=True)
        return head

    assert branch_name not in repo.heads
    try:
        new_head = repo.create_head(branch_name, from_ref)
    except Exception:
        msg = f"unable to create new head '{branch_name}' " + f"from '{from_ref}'"
        logger.exception(msg)
        raise ApplyError(msg=msg) from None

    repo.head.reference = new_head
    _ = repo.head.reset(index=True, working_tree=True)

    try:
        git_cleanup_repo(repo_path)
        _update_submodules(repo_path)
    except Exception as e:
        msg = f"unable to clean up repo state after checkout: {e}"
        logger.error(msg)
        raise ApplyError(msg=msg) from None

    return new_head


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
    _update_submodules(repo_path)


def apply_manifest(
    manifest: ReleaseManifest,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    target_branch: str,
    token: str,
    *,
    no_cleanup: bool = False,
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
        _ = git_prepare_remote(
            ceph_repo_path, f"github.com/{repo_name}", repo_name, token
        )
    except ApplyError as e:
        logger.error(e)
        raise e from None

    try:
        _branch = _checkout_ref(ceph_repo_path, manifest.base_ref, target_branch)
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
    )
