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
import sys
import uuid
from datetime import datetime as dt
from pathlib import Path
from typing import override

import git
from crtlib.db import ReleasesDB
from crtlib.errors.patchset import (
    PatchSetCheckError,
    PatchSetExistsError,
)
from crtlib.git import (
    SHA,
    GitCherryPickConflictError,
    GitCherryPickError,
    GitEmptyPatchDiffError,
    GitPatchDiffError,
    git_abort_cherry_pick,
    git_check_patches_diff,
    git_cherry_pick,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patch import Patch
from crtlib.models.patchset import (
    GitHubPullRequest,
    PatchSetBase,
)
from crtlib.patchset import (
    patchset_check_patches_diff,
)

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


def _check_patch_needed(repo_path: Path, sha: str) -> int:
    try:
        added, skipped = git_check_patches_diff(
            repo_path, "HEAD", sha, limit=f"{sha}~1"
        )
    except GitEmptyPatchDiffError:
        logger.warning(f"patch sha '{sha}' diff with HEAD is empty")
        return 0
    except GitPatchDiffError as e:
        msg = f"unable to check if patch sha '{sha}' is needed in HEAD: {e}"
        logger.error(msg)
        raise ApplyError(msg=msg) from None

    if len(added) + len(skipped) > 1:
        msg = (
            f"unexpected number of patches needed for sha '{sha}': added '{added}' "
            + "skipped '{skipped}'"
        )
        logger.error(msg)
        raise ApplyError(msg=msg)

    return 1 if added else -1 if skipped else 0


def _prepare_remote(repo: git.Repo, token: str, org: str, repo_name: str) -> git.Remote:
    remote_name = f"{org}/{repo_name}"
    logger.debug(f"prepare remote name '{remote_name}'")
    try:
        remote = repo.remote(remote_name)
    except ValueError:
        remote_url = f"https://ceph-release-tool:{token}@github.com/{org}/{repo_name}"
        remote = repo.create_remote(remote_name, remote_url)
        logger.debug(f"create remote name: {remote_name}, url: {remote_url}")

    logger.debug(f"update remote name '{remote_name}'")
    _ = remote.update()
    return remote


def _checkout_ref(repo: git.Repo, from_ref: str, branch_name: str) -> git.Head:
    logger.debug(f"checkout ref '{from_ref}' to '{branch_name}'")
    assert branch_name not in repo.heads
    try:
        new_head = repo.create_head(branch_name, from_ref)
    except Exception:
        msg = f"unable to create new head '{branch_name}' " + f"from '{from_ref}'"
        logger.exception(msg)
        raise ApplyError(msg=msg) from None

    repo.head.reference = new_head
    _ = repo.head.reset(index=True, working_tree=True)
    return new_head


def _prepare_repo(repo: git.Repo):
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
                logger.exception(msg)
                raise ApplyError(msg=msg) from None

            if not res:
                msg = f"user's {what} not set for repository"
                logger.error(msg)
                raise ApplyError(msg=msg)

    def _prepare_repo() -> None:
        logger.debug("update submodules")
        try:
            repo.git.execute(  # pyright: ignore[reportCallIssue]
                ["git", "submodule", "update", "--init", "--recursive"],
                output_stream=sys.stdout.buffer,
                as_process=False,
                with_stdout=True,
            )
        except Exception as e:
            msg = f"unable to update repository's submodules: {e}"
            logger.error(msg)
            raise ApplyError(msg=msg) from None

    # propagate exceptions
    _check_repo()
    _prepare_repo()


def _prepare_patchsets(
    db: ReleasesDB,
    repo_path: Path,
    token: str,
    patchset_uuid_lst: list[uuid.UUID],
    base_ref: str,
) -> list[GitHubPullRequest]:
    logger.debug("prepare patchset list from manifest")

    repo = git.Repo(repo_path)

    patchset_lst: list[GitHubPullRequest] = []
    for patchset_uuid in patchset_uuid_lst:
        try:
            patchset = db.load_patchset(patchset_uuid)
        except Exception as e:
            raise e from None

        if not isinstance(patchset, GitHubPullRequest):
            logger.debug(
                f"patchset uuid '{patchset.patchset_uuid}' not a github patchset"
            )
            continue

        patchset_lst.append(patchset)
        remote = _prepare_remote(repo, token, patchset.org_name, patchset.repo_name)
        pr_id = patchset.pull_request_id
        src_ref = f"pull/{pr_id}/head"
        dst_ref = f"patchset/gh/{patchset.org_name}/{patchset.repo_name}/{pr_id}"
        _ = remote.fetch(f"{src_ref}:{dst_ref}")

        try:
            _ = patchset_check_patches_diff(repo_path, patchset, dst_ref, base_ref)
        except PatchSetCheckError as e:
            msg = f"unable to check patchset patch diff: {e}"
            logger.error(msg)
            raise ApplyError(msg=msg) from None

    return patchset_lst


def apply_manifest(
    db: ReleasesDB,
    manifest: ReleaseManifest,
    ceph_git_path: Path,
    token: str,
    target_branch: str,
    *,
    no_cleanup: bool = False,
) -> tuple[bool, list[Patch], list[Patch]]:
    repo = git.Repo(ceph_git_path)

    logger.debug(
        f"apply manifest '{manifest.release_uuid}' to branch '{target_branch}'"
    )

    def _cleanup(*, abort_cherrypick: bool = False) -> None:
        logger.debug(f"cleanup state, branch '{target_branch}'")
        if abort_cherrypick:
            git_abort_cherry_pick(ceph_git_path)

        repo.head.reference = repo.heads.main
        repo.git.branch("-D", target_branch)  # pyright: ignore[reportAny]

    def _apply_patchsets(
        patchsets: list[GitHubPullRequest],
    ) -> tuple[list[Patch], list[Patch]]:
        logger.debug(f"apply {len(patchsets)} patchsets")

        skipped: list[Patch] = []
        added: list[Patch] = []

        for patchset in patchsets:
            logger.debug(
                f"apply patch set uuid '{patchset.patchset_uuid}', "
                + f"pr id '{patchset.pull_request_id}'"
            )
            for patch in patchset.patches:
                logger.debug(f"apply patch uuid '{patch.patch_uuid}' sha '{patch.sha}'")

                if _check_patch_needed(ceph_git_path, patch.sha) <= 0:
                    logger.info(
                        f"patch uuid '{patch.patch_uuid}' sha '{patch.sha}' skipped"
                    )
                    skipped.append(patch)
                    continue

                try:
                    git_cherry_pick(ceph_git_path, patch.sha)
                except GitCherryPickConflictError as e:
                    raise e from None
                except GitCherryPickError as e:
                    msg = (
                        f"unable to cherry-pick uuid '{patch.patch_uuid}' "
                        + f"sha '{patch.sha}': {e}"
                    )
                    logger.error(msg)
                    raise ApplyError(msg=msg) from None

                added.append(patch)

        return (added, skipped)

    try:
        _prepare_repo(repo)
        _remote = _prepare_remote(
            repo, token, manifest.base_ref_org, manifest.base_ref_repo
        )
        _branch = _checkout_ref(repo, manifest.base_ref, target_branch)
        patchsets = _prepare_patchsets(
            db, ceph_git_path, token, manifest.patchsets, manifest.base_ref
        )
    except ApplyError as e:
        msg = f"unable to apply manifest patchsets: {e}"
        logger.error(msg)
        if not no_cleanup:
            _cleanup()

        raise ApplyError(msg=msg) from e

    abort_cherrypick = True
    try:
        added, skipped = _apply_patchsets(patchsets)
    except GitCherryPickConflictError as e:
        raise ApplyConflictError(e.sha, e.conflicts) from None
    except ApplyError as e:
        logger.error(f"unable to apply patchsets to '{target_branch}': {e}")
        return (False, [], [])
    else:
        abort_cherrypick = False
    finally:
        if not no_cleanup:
            _cleanup(abort_cherrypick=abort_cherrypick)

    return (len(added) > 0, added, skipped)


def patchset_apply_to_manifest(
    db: ReleasesDB,
    orig_manifest: ReleaseManifest,
    patchset: PatchSetBase,
    repo_path: Path,
    token: str,
) -> tuple[bool, list[Patch], list[Patch]]:
    manifest = orig_manifest.model_copy(deep=True)
    if not manifest.add_patchset(patchset):
        raise PatchSetExistsError(msg=f"uuid '{patchset.patchset_uuid}'")

    seq = dt.now(datetime.UTC).strftime("%Y%m%dT%H%M%S")
    target_branch = f"{manifest.name}-{manifest.release_git_uid}-{seq}"

    # propagate exceptions to caller
    return apply_manifest(
        db,
        manifest,
        repo_path,
        token,
        target_branch,
        no_cleanup=False,
    )
