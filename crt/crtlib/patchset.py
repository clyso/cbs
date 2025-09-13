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


import datetime
import uuid
from datetime import datetime as dt
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
    git_branch_delete,
    git_check_patches_diff,
    git_format_patch,
    git_prepare_remote,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.common import ManifestPatchEntry
from crtlib.models.discriminator import (
    ManifestPatchEntryWrapper,
)
from crtlib.models.patch import Patch, PatchMeta
from crtlib.models.patchset import (
    CustomPatchSet,
    GitHubPullRequest,
    PatchSetBase,
)
from crtlib.patch import PatchError, parse_formatted_patch_info, patch_import

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


def load_patchset(
    patches_repo_path: Path, patchset_uuid: uuid.UUID
) -> ManifestPatchEntry:
    """Load a patch set from the patches repository by its UUID."""
    patchset_meta_path = (
        patches_repo_path.joinpath("ceph")
        .joinpath("patches")
        .joinpath("meta")
        .joinpath(f"{patchset_uuid}.json")
    )
    if not patchset_meta_path.exists():
        msg = f"missing patch set meta for uuid '{patchset_uuid}'"
        logger.error(msg)
        raise NoSuchPatchSetError()

    try:
        wrapped_entry = ManifestPatchEntryWrapper.model_validate_json(
            patchset_meta_path.read_text()
        )
        entry = wrapped_entry.contents
    except pydantic.ValidationError as e:
        msg = f"malformed patch set uuid '{patchset_uuid}'"
        logger.error(msg)
        logger.error(str(e))
        raise MalformedPatchSetError(msg=msg) from None
    except Exception as e:
        msg = f"unexpected error obtaining patch set meta uuid '{patchset_uuid}': {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    return entry


def get_patchset_meta_path(patches_repo_path: Path, patchset_uuid: uuid.UUID) -> Path:
    return patches_repo_path / "ceph" / "patches" / "meta" / f"{patchset_uuid}.json"


ManifestPatchEntryTypes = GitHubPullRequest | PatchMeta | CustomPatchSet


def write_patchset(patches_repo_path: Path, patchset: ManifestPatchEntryTypes) -> None:
    """Write a patch set's meta file into the patches repository."""
    patchset_meta_path = get_patchset_meta_path(patches_repo_path, patchset.entry_uuid)
    patchset_meta_path.parent.mkdir(parents=True, exist_ok=True)

    try:
        _ = patchset_meta_path.write_text(
            ManifestPatchEntryWrapper(contents=patchset).model_dump_json(indent=2)
        )
    except Exception as e:
        msg = f"unable to write patch set meta file: {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    logger.debug(f"wrote patch set meta file '{patchset_meta_path}'")


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

    # propagate exceptions
    entry = load_patchset(patches_repo_path, patchset_uuid)

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
        write_patchset(patches_repo_path, patchset)
        _set_latest(patchset_pr_dir_path, patchset_head_sha)
    except Exception as e:
        msg = f"error writing patch set '{pr_id}' from '{repo_path}': {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    logger.debug(
        f"obtained patch set '{repo_path}#{pr_id}' latest '{patchset_head_sha}'"
    )


def _formatted_patch_to_patch(repo: str, sha: str, title: str, raw_patch: str) -> Patch:
    try:
        patch_info = parse_formatted_patch_info(raw_patch)
    except PatchError as e:
        logger.error(f"unable to parse formatted patch info: {e}")
        raise e from None

    return Patch(
        sha=sha,
        author=patch_info.author,
        author_date=patch_info.date,
        commit_author=None,
        commit_date=None,
        title=title,
        message=patch_info.desc,
        cherry_picked_from=patch_info.cherry_picked_from,
        related_to=patch_info.fixes,
        parent="",
        repo_url=f"https://github.com/{repo}",
        patch_id="",
        patchset_uuid=None,
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


def fetch_custom_patchset_patches(
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patchset: CustomPatchSet,
    token: str,
) -> list[Patch]:
    """Fetch and store a custom patch set's patches into the patches repository."""
    if patchset.is_published:
        raise PatchSetError(msg="cannot fetch published custom patch set")

    if not patchset.patches_meta:
        raise PatchSetError(msg="empty custom patch set")

    patchset_path = (
        patches_repo_path / "ceph" / "patches" / f"{patchset.entry_uuid}.patch"
    )
    if patchset_path.exists():
        msg = f"custom patch set '{patchset.entry_uuid}' already exists"
        logger.error(msg)
        raise PatchSetError(msg=msg)
    patchset_path.parent.mkdir(exist_ok=True, parents=True)

    patches: list[Patch] = []
    # prepare all remotes in this patch set, so we don't have to update them
    # individually per meta entry, and obtain the patch sets' branches.
    fetched_branches: set[str] = set()
    seq = dt.now(datetime.UTC).strftime("%Y%m%d%H%M%S")
    for meta in patchset.patches_meta:
        dst_branch = (
            f"patchset/branch/{meta.repo.replace('/', '--')}--{meta.branch}-{seq}"
        )
        if dst_branch in fetched_branches:
            continue

        try:
            remote = git_prepare_remote(
                ceph_repo_path, f"github.com/{meta.repo}", meta.repo, token
            )
            _ = remote.fetch(refspec=f"{meta.branch}:{dst_branch}")
        except Exception as e:
            msg = (
                f"error fetching patchset branch '{meta.branch}' "
                + f"from '{meta.repo}': {e}"
            )
            logger.error(msg)
            raise PatchSetError(msg=msg) from None

        fetched_branches.add(dst_branch)

    def _cleanup() -> None:
        for branch in fetched_branches:
            try:
                git_branch_delete(ceph_repo_path, branch)
            except Exception as e:
                msg = f"unable to delete temporary branch '{branch}': {e}"
                logger.error(msg)
                raise PatchSetError(msg=msg) from None

    patchset_formatted_patches: list[str] = []
    for meta in patchset.patches_meta:
        interval_str = f"[{meta.sha}, {meta.sha_end}]" if meta.sha_end else meta.sha
        logger.debug(f"format patches '{interval_str}' from '{meta.repo}'")
        for patch in meta.patches:
            sha = patch[0]
            title = patch[1]
            logger.debug(f"format patch sha '{sha}' title '{title}'")
            try:
                formatted_patch = git_format_patch(ceph_repo_path, sha)
            except GitError as e:
                _cleanup()
                msg = f"unable to obtain formatted patch for sha '{sha}': {e}"
                logger.error(msg)
                raise PatchSetError(msg=msg) from None

            patchset_formatted_patches.append(formatted_patch)
            patch_data = _formatted_patch_to_patch(
                meta.repo, sha, title, formatted_patch
            )
            patch_data.commit_author = patchset.author
            patch_data.commit_date = patchset.creation_date
            patch_data.patchset_uuid = patchset.entry_uuid
            patches.append(patch_data)

    logger.debug(f"write '{len(patchset_formatted_patches)}' patches")
    try:
        with patchset_path.open("w", encoding="utf-8") as f:
            _ = f.write("\n".join(patchset_formatted_patches))
    except Exception as e:
        _cleanup()
        msg = f"unable to write custom patch set '{patchset.entry_uuid}': {e}"
        logger.error(msg)
        raise PatchSetError(msg=msg) from None

    _cleanup()
    return patches


def fetch_patchset_patches_from_repo(
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patchset: PatchSetBase,
    token: str,
    *,
    force: bool = False,
) -> list[Patch]:
    if isinstance(patchset, GitHubPullRequest):
        patchset_fetch_gh_patches(
            ceph_repo_path,
            patches_repo_path,
            patchset,
            token,
            force=force,
        )
        return patchset.patches
    elif isinstance(patchset, CustomPatchSet):
        return fetch_custom_patchset_patches(
            ceph_repo_path, patches_repo_path, patchset, token
        )

    else:
        raise PatchSetError(f"unsupported patch set type: {type(patchset)}")
