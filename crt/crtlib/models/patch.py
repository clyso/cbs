# crt - models - patch
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
from datetime import datetime as dt
from typing import override

import pydantic
from crtlib.git_utils import SHA
from crtlib.models.common import (
    AuthorData,
    ManifestPatchEntry,
    ManifestPatchSetEntryType,
    patch_canonical_title,
)


class Patch(pydantic.BaseModel):
    """Represents a singular patch."""

    sha: SHA
    author: AuthorData
    author_date: dt
    commit_author: AuthorData | None
    commit_date: dt | None
    title: str
    message: str
    cherry_picked_from: list[str]
    related_to: list[str]
    parent: SHA

    repo_url: str
    patch_id: SHA
    patch_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    patchset_uuid: uuid.UUID | None


class PatchInfo(pydantic.BaseModel):
    author: AuthorData
    date: dt
    title: str
    desc: str
    signed_off_by: list[AuthorData]
    cherry_picked_from: list[str]
    fixes: list[str]


class PatchMeta(ManifestPatchEntry):
    sha: SHA
    patch_id: SHA
    src_version: str | None
    info: PatchInfo

    @override
    def _get_entry_type(self) -> ManifestPatchSetEntryType:
        return ManifestPatchSetEntryType.SINGLE

    @override
    def _get_canonical_title(self) -> str:
        return patch_canonical_title(self.info.title)

    @override
    def _get_repr(self) -> str:
        return f"single patch set uuid {self.entry_uuid} ({self.sha})"
