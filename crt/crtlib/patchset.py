# crt - patch set
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


import uuid
from pathlib import Path

import pydantic
from crtlib.errors.patchset import (
    MalformedPatchSetError,
    NoSuchPatchSetError,
    PatchSetCheckError,
    PatchSetError,
)
from crtlib.git_utils import (
    GitEmptyPatchDiffError,
    GitError,
    GitPatchDiffError,
    git_check_patches_diff,
    git_format_patch,
    git_prepare_remote,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.discriminator import (
    ManifestPatchEntryWrapper,
)
from crtlib.models.patch import Patch
from crtlib.models.patchset import GitHubPullRequest, PatchSetBase
from crtlib.patch import PatchError, patch_import

logger = parent_logger.getChild("patchset")


def patchset_check_patches_diff(
    ceph_git_path: Path, patchset: PatchSetBase, patchset_branch: str, base_ref: str
) -> tuple[list[str], list[str]] | None:
    logger.debug(f"check patchset branch '{patchset_branch}' against '{base_ref}'")

    try:
        added, skipped = git_check_patches_diff(
            ceph_git_path, base_ref, patchset_branch, limit=patchset.base_sha
        )
    except GitEmptyPatchDiffError:
        logger.warning(
            f"empty patch diff between patchset '{patchset_branch}' and '{base_ref}'"
        )
        return None
    except GitPatchDiffError as e:
        msg = f"unable to check patchset '{patchset_branch}' against '{base_ref}': {e}"
        logger.error(msg)
        raise PatchSetCheckError(msg=msg) from None

    logger.debug(f"patchset '{patchset_branch}' add {added}")
    logger.debug(f"patchset '{patchset_branch}' drop {skipped}")

    if len(added) + len(skipped) != len(patchset.patches):
        msg = "missing patches from patchset diff"
        logger.error(msg)
        raise PatchSetCheckError(msg=msg)

    return (added, skipped)


def patchset_import_patches(
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patches: list[Patch],
    target_version: str,
) -> None:
    for patch in patches:
        try:
            patch_import(
                patches_repo_path,
                ceph_repo_path,
                patch.sha,
                target_version=target_version,
            )
        except PatchError as e:
            msg = f"unable to import patch sha '{patch.sha}': {e}"
            logger.error(msg)
            raise PatchSetError(msg=msg) from None

        logger.info(f"imported patch set's patch sha '{patch.sha}'")


def patchset_get_gh(
    patches_repo_path: Path, repo_owner: str, repo_name: str, pr_id: int
) -> GitHubPullRequest:
    """Obtain a github pull request's latest meta file from the patches repository."""
    patchset_pr_path = (
        patches_repo_path.joinpath("ceph")
        .joinpath("patches")
        .joinpath(repo_owner)
        .joinpath(repo_name)
        .joinpath(str(pr_id))
        .joinpath("latest")
    )
    if not patchset_pr_path.exists():
        raise NoSuchPatchSetError()

    try:
        patchset_uuid_str = patchset_pr_path.read_text()
    except Exception as e:
        msg = f"error reading patch set uuid: {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    if not patchset_uuid_str:
        msg = "unexpected missing patch set uuid"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    try:
        patchset_uuid = uuid.UUID(patchset_uuid_str)
    except Exception:
        msg = f"unexpected malformed patch set uuid: '{patchset_uuid_str}'"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    patchset_meta_path = (
        patches_repo_path.joinpath("ceph")
        .joinpath("patches")
        .joinpath("meta")
        .joinpath(f"{patchset_uuid}.json")
    )
    if not patchset_meta_path.exists():
        msg = f"missing patch set meta for uuid '{patchset_uuid}'"
        logger.error(msg)
        raise PatchSetError(msg=msg)

    try:
        wrapped_entry = ManifestPatchEntryWrapper.model_validate_json(
            patchset_meta_path.read_text()
        )
        entry = wrapped_entry.contents
    except pydantic.ValidationError as e:
        msg = f"malformed gh patch set uuid '{patchset_uuid}'"
        logger.error(msg)
        logger.error(str(e))
        raise MalformedPatchSetError(msg=msg) from None
    except Exception as e:
        msg = f"unexpected error obtaining patch set meta uuid '{patchset_uuid}': {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    if not isinstance(entry, GitHubPullRequest):
        raise PatchSetError(msg=f"wrong patch set type: {type(entry)}")
    return entry


def patchset_fetch_gh_patches(
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patchset: GitHubPullRequest,
    token: str,
    *,
    force: bool = False,
) -> None:
    """Fetch and store a GitHub pull request's patches into the patches repository."""

    def _set_latest(pr_dir_path: Path, sha: str) -> None:
        logger.debug(f"set latest patch set '{pr_dir_path}' to '{sha}'")
        ln_path = pr_dir_path / "latest"
        if ln_path.exists():
            ln_path.unlink()

        ln_path.symlink_to(sha)

    if not patchset.patches:
        raise PatchSetError(msg="empty patch set")

    repo_path = f"{patchset.org_name}/{patchset.repo_name}"
    pr_id = patchset.pull_request_id

    patchset_pr_dir_path = (
        patches_repo_path
        / "ceph"
        / "patches"
        / patchset.org_name
        / patchset.repo_name
        / str(pr_id)
    )
    patchset_pr_dir_path.mkdir(exist_ok=True, parents=True)

    patchset_meta_path = (
        patches_repo_path / "ceph" / "patches" / "meta" / f"{patchset.entry_uuid}.json"
    )
    patchset_meta_path.parent.mkdir(exist_ok=True, parents=True)

    patchset_path = (
        patches_repo_path / "ceph" / "patches" / f"{patchset.entry_uuid}.patch"
    )
    patchset_path.parent.mkdir(exist_ok=True, parents=True)

    # check if the pull request's head patch already exists.
    patchset_head_sha = patchset.patches[-1].sha
    patchset_head_path = patchset_pr_dir_path / patchset_head_sha
    if patchset_head_path.exists() and not force:
        logger.debug(
            f"patch set '{repo_path}#{pr_id}' head '{patchset_head_sha}' already exists"
        )
        _set_latest(patchset_pr_dir_path, patchset_head_sha)
        return

    # obtain patches
    remote = git_prepare_remote(
        ceph_repo_path, f"github.com/{repo_path}", repo_path, token
    )
    src_ref = f"pull/{pr_id}/head"
    dst_ref = f"patchset/gh/{repo_path}/{pr_id}"
    try:
        _ = remote.fetch(f"{src_ref}:{dst_ref}")
    except Exception as e:
        msg = f"error fetching patchset '{pr_id}' from '{repo_path}': {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    try:
        formatted_patchset = git_format_patch(
            ceph_repo_path,
            patchset.head_sha,
            base_rev=patchset.base_sha,
        )
    except GitError as e:
        msg = f"error formatting patch set: {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    try:
        _ = patchset_path.write_text(formatted_patchset)
        _ = patchset_head_path.write_text(str(patchset.entry_uuid))
        _ = patchset_meta_path.write_text(
            ManifestPatchEntryWrapper(contents=patchset).model_dump_json(indent=2)
        )
        _set_latest(patchset_pr_dir_path, patchset_head_sha)
    except Exception as e:
        msg = f"error writing patch set '{pr_id}' from '{repo_path}': {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    logger.debug(
        f"obtained patch set '{repo_path}#{pr_id}' latest '{patchset_head_sha}'"
    )


def patchset_from_gh_needs_update(
    existing: GitHubPullRequest, new: GitHubPullRequest
) -> bool:
    """
    Check if a given existing GitHub pull request needs to be updated.

    Compares the existing pull request state to its upstream state. If a pull
    request has been merged, then there's nothing to update.
    """
    if existing.merged:
        return False

    assert new.updated_date

    if existing.updated_date:
        return (existing.updated_date - new.updated_date).total_seconds() < 0

    return True
