# crt - release manifests
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

from crtlib.apply import ApplyError, apply_manifest
from crtlib.db import ReleasesDB
from crtlib.errors.manifest import ManifestError
from crtlib.git_utils import (
    GitError,
    GitFetchError,
    GitFetchHeadNotFoundError,
    GitIsTagError,
    GitPushError,
    git_checkout_ref,
    git_cleanup_repo,
    git_fetch_ref,
    git_prepare_remote,
    git_push,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patch import Patch

logger = parent_logger.getChild("manifest")


class ManifestExecuteResult:
    applied: bool
    target_branch: str
    added: list[Patch]
    skipped: list[Patch]

    pushed_to_remote: bool
    remote_updated: bool
    heads_updated: list[str]
    heads_rejected: list[str]

    def __init__(
        self,
        *,
        applied: bool,
        target_branch: str,
        added: list[Patch],
        skipped: list[Patch],
        pushed_to_remote: bool,
        remote_updated: bool,
        heads_updated: list[str],
        heads_rejected: list[str],
    ) -> None:
        self.applied = applied
        self.target_branch = target_branch
        self.added = added
        self.skipped = skipped
        self.pushed_to_remote = pushed_to_remote
        self.remote_updated = remote_updated
        self.heads_updated = heads_updated
        self.heads_rejected = heads_rejected


def _prepare_repo(
    repo_path: Path,
    manifest_uuid: uuid.UUID,
    base_ref: str,
    target_branch: str,
    base_remote_name: str,
    push_remote_name: str,
    token: str,
) -> None:
    try:
        git_cleanup_repo(repo_path)
    except GitError as e:
        msg = f"unable to clean up repository: {e}"
        logger.error(msg)
        raise ManifestError(manifest_uuid, msg) from None

    try:
        base_remote_uri = f"github.com/{base_remote_name}"
        _ = git_prepare_remote(repo_path, base_remote_uri, base_remote_name, token)
        push_remote_uri = f"github.com/{push_remote_name}"
        _ = git_prepare_remote(repo_path, push_remote_uri, push_remote_name, token)
    except GitError as e:
        raise ManifestError(manifest_uuid, msg=str(e)) from None

    # fetch from base repository, if it exists.
    try:
        _ = git_fetch_ref(repo_path, target_branch, target_branch, push_remote_name)
    except GitIsTagError as e:
        msg = f"unexpected tag for branch '{target_branch}': {e}"
        logger.error(msg)
        raise ManifestError(manifest_uuid, msg) from None
    except GitFetchHeadNotFoundError:
        # does not exist in the provided remote.
        logger.info(
            f"branch '{target_branch}' does not exist in remote '{push_remote_name}'"
        )
    except GitFetchError as e:
        msg = f"unable to fetch '{target_branch}' from '{push_remote_name}': {e}"
        logger.error(msg)
        raise ManifestError(manifest_uuid, msg=msg) from None
    except GitError as e:
        msg = (
            f"unexpected error fetching branch '{target_branch}' "
            + f"from '{push_remote_name}': {e}"
        )
        logger.error(msg)
        raise ManifestError(manifest_uuid, msg) from None

    # we either fetched and thus we have an up-to-date local branch, or we didn't find
    # a corresponding reference in the remote and we need to either:
    #  1. checkout a new copy of the base ref to the target branch
    #  2. use an existing local target branch
    try:
        _ = git_checkout_ref(
            repo_path,
            base_ref,
            to_branch=target_branch,
            remote_name=base_remote_name,
            update_from_remote=False,
            fetch_if_not_exists=True,
        )
    except GitError as e:
        msg = f"unable to checkout ref '{base_ref}' to '{target_branch}': {e}"
        logger.error(msg)
        raise ManifestError(manifest_uuid, msg) from None

    logger.debug(f"checked out '{target_branch}'")

    pass


def manifest_execute(
    db: ReleasesDB,
    manifest: ReleaseManifest,
    repo_path: Path,
    token: str,
    push: bool,
) -> ManifestExecuteResult:
    """
    Execute a manifest against its base ref.

    If the target branch for this manifest exists locally, attempt to fetch changes
    from the base repository (if it exists). Then execute the manifest against the
    target branch.

    If the target branch for this manifest exists in the manifest's base repository,
    checkout said branch and execute the manifest against it.

    If the target branch doesn't exist at all, checkout the branch from the manifest's
    base ref and execute the manifest against it.
    """
    base_remote_name = f"{manifest.base_ref_org}/{manifest.base_ref_repo}"
    logger.info(
        f"execute manifest '{manifest.release_uuid}' for repo '{base_remote_name}'"
    )

    target_branch = f"{manifest.name}-{manifest.release_git_uid}"
    logger.debug(f"execute manifest on branch '{target_branch}'")

    try:
        _prepare_repo(
            repo_path,
            manifest.release_uuid,
            manifest.base_ref,
            target_branch,
            base_remote_name,
            manifest.dst_repo,
            token,
        )
    except ManifestError as e:
        logger.error(f"unable to prepare repository to execute manifest: {e}")
        raise e from None

    # apply manifest to currently checked out branch
    try:
        res, added, skipped = apply_manifest(
            db,
            manifest,
            repo_path,
            token,
            target_branch,
            no_cleanup=True,
        )
        pass
    except ApplyError as e:
        msg = f"unable to apply manifest to '{target_branch}': {e}"
        logger.error(msg)
        raise ManifestError(manifest.release_uuid, msg) from None

    logger.debug(
        f"applied manifest: {res}, added '{len(added)}' "
        + f"skipped '{len(skipped)}' patches"
    )

    push_res = False
    heads_updated: list[str] = []
    heads_rejected: list[str] = []
    if push:
        logger.info(f"push '{target_branch}' to '{manifest.dst_repo}'")
        try:
            push_res, heads_updated, heads_rejected = git_push(
                repo_path, target_branch, manifest.dst_repo
            )
        except GitPushError as e:
            msg = f"unable to push '{target_branch}': {e}"
            logger.error(msg)
            raise ManifestError(manifest.release_uuid, msg) from None
        except GitError as e:
            msg = f"unexpected error pushing '{target_branch}': {e}"
            logger.error(msg)
            raise ManifestError(manifest.release_uuid, msg) from None

        if not res:
            logger.info(
                f"branch '{target_branch}' not updated on remote '{manifest.dst_repo}'"
            )

        logger.info(f"updated heads: {heads_updated}")
        logger.info(f"rejected heads: {heads_rejected}")

    return ManifestExecuteResult(
        applied=res,
        target_branch=target_branch,
        added=added,
        skipped=skipped,
        pushed_to_remote=push,
        remote_updated=push_res,
        heads_updated=heads_updated,
        heads_rejected=heads_rejected,
    )
