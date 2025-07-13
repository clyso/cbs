# crt - models - patchset
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
from datetime import datetime as dt
from typing import Annotated, Any, override

import pydantic
from crtlib.errors.patchset import EmptyPatchSetError
from crtlib.git_utils import SHA
from crtlib.models.common import (
    AuthorData,
    ManifestPatchEntry,
    ManifestPatchSetEntryType,
    patch_canonical_title,
)
from crtlib.models.patch import Patch


class PatchSetBase(ManifestPatchEntry, abc.ABC):
    """Represents a set of related patches."""

    author: AuthorData
    creation_date: dt
    title: str
    related_to: list[str]
    patches: list[Patch]

    @property
    def base_sha(self) -> SHA:
        if not self.patches:
            raise EmptyPatchSetError(str(self.entry_uuid))

        first_patch = next(iter(self.patches))
        return first_patch.parent

    @property
    def head_sha(self) -> SHA:
        if not self.patches:
            raise EmptyPatchSetError(str(self.entry_uuid))

        last_patch = next(reversed(self.patches))
        return last_patch.sha


class GitHubPullRequest(PatchSetBase):
    """Represents a GitHub Pull Request, containing one or more patches."""

    org_name: str
    repo_name: str
    repo_url: str
    pull_request_id: int
    merge_date: dt | None
    merged: bool
    target_branch: str

    @override
    def _get_entry_type(self) -> ManifestPatchSetEntryType:
        return ManifestPatchSetEntryType.PATCHSET_GITHUB

    @override
    def _get_canonical_title(self) -> str:
        patch_title = patch_canonical_title(self.title)
        return f"[{self.pull_request_id}]-{patch_title}"

    @override
    def compute_hash_bytes(self) -> bytes:
        return self.model_dump_json().encode()


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
