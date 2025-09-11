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


class CustomPatchMeta(pydantic.BaseModel):
    repo: str
    branch: str
    sha: SHA
    sha_end: SHA | None = pydantic.Field(default=None)
    patches: list[tuple[SHA, str]] = pydantic.Field(default=[])


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
    updated_date: dt | None = pydantic.Field(default=None)
    merge_date: dt | None
    merged: bool
    target_branch: str

    @override
    def _get_entry_type(self) -> ManifestPatchSetEntryType:
        return ManifestPatchSetEntryType.PATCHSET_GITHUB

    @override
    def _get_canonical_title(self) -> str:
        patch_title = patch_canonical_title(self.title)
        return (
            f"[{self.org_name}\\{self.repo_name}#{self.pull_request_id}]-{patch_title}"
        )


class CustomPatchSet(PatchSetBase):
    """Represents a custom patch set, created by the user."""

    description: str | None = pydantic.Field(default=None)
    release_name: str | None = pydantic.Field(default=None)
    patches_meta: list[CustomPatchMeta] = pydantic.Field(default=[])
    is_published: bool = pydantic.Field(default=False)

    @override
    def _get_entry_type(self) -> ManifestPatchSetEntryType:
        return ManifestPatchSetEntryType.PATCHSET_CUSTOM

    @override
    def _get_canonical_title(self) -> str:
        patch_title = patch_canonical_title(self.title)
        patch_prefix = self.release_name or "generic"
        return f"[{patch_prefix}]-{patch_title}"

    @property
    def description_text(self) -> str | None:
        if not self.description:
            return None

        lines = self.description.splitlines()
        # Drop the first line
        lines = lines[1:]
        # Drop lines starting with 'signed-off-by'
        # (case-insensitive, leading spaces allowed)
        filtered = [
            line
            for line in lines
            if line and not line.lstrip().lower().startswith("signed-off-by")
        ]
        return "\n".join(filtered).strip() if filtered else None


def _patchset_discriminator(v: Any) -> str:  # pyright: ignore[reportExplicitAny, reportAny]
    if isinstance(v, GitHubPullRequest):
        return "gh"
    elif isinstance(v, CustomPatchSet):
        return "custom"
    elif isinstance(v, dict):
        if "pull_request_id" in v:
            return "gh"
        elif "release_name" in v:
            return "custom"
        else:
            return "vanilla"
    else:
        return "vanilla"


class PatchSet(pydantic.BaseModel):
    info: Annotated[
        Annotated[GitHubPullRequest, pydantic.Tag("gh")]
        | Annotated[PatchSetBase, pydantic.Tag("vanilla")]
        | Annotated[CustomPatchSet, pydantic.Tag("custom")],
        pydantic.Discriminator(_patchset_discriminator),
    ]
