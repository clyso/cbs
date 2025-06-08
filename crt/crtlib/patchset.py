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


import abc
import uuid
from datetime import datetime as dt
from pathlib import Path
from typing import Annotated, Any, override

import pydantic
from crtlib.git import GitEmptyPatchDiffError, GitPatchDiffError, git_check_patches_diff
from crtlib.logger import logger as parent_logger
from crtlib.patch import AuthorData, Patch

logger = parent_logger.getChild("patchset")


class PatchSetError(Exception):
    msg: str

    def __init__(self, msg: str) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return f"patch set error: {self.msg}"


class NoSuchPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"patch set does not exists: {self.msg}"


class MalformedPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"malformed patch set: {self.msg}"


class PatchSetMismatchError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"mismatch patch set type: {self.msg}"


class PatchSetCheckError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"patch set check error: {self.msg}"


class PatchSetBase(pydantic.BaseModel, abc.ABC):  # pyright: ignore[reportUnsafeMultipleInheritance]
    """Represents a set of related patches."""

    author: AuthorData
    creation_date: dt
    title: str
    related_to: list[str]
    patches: list[Patch]

    patchset_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())


class GitHubPullRequest(PatchSetBase):
    """Represents a GitHub Pull Request, containing one or more patches."""

    org_name: str
    repo_name: str
    repo_url: str
    pull_request_id: int
    merge_date: dt | None
    merged: bool
    target_branch: str


def _patchset_discriminator(v: Any) -> str:  # pyright: ignore[reportExplicitAny, reportAny]
    if isinstance(v, GitHubPullRequest):
        return "gh"
    elif isinstance(v, dict):
        if "pull_request_id" in v:
            return "gh"
        else:
            return "vanilla"
    else:
        return "vanilla"


class PatchSet(pydantic.BaseModel):
    info: Annotated[
        Annotated[GitHubPullRequest, pydantic.Tag("gh")]
        | Annotated[PatchSetBase, pydantic.Tag("vanilla")],
        pydantic.Discriminator(_patchset_discriminator),
    ]


def patchset_check_patches_diff(
    ceph_git_path: Path, patchset: PatchSetBase, patchset_branch: str, base_ref: str
) -> tuple[list[str], list[str]] | None:
    logger.debug(f"check patchset branch '{patchset_branch}' against '{base_ref}'")

    try:
        added, skipped = git_check_patches_diff(
            ceph_git_path, base_ref, patchset_branch
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
