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

import datetime
import string
import uuid
from datetime import datetime as dt
from random import choices
from typing import override

import pydantic
from crtlib.patchset import PatchSetBase


class ManifestError(Exception):
    manifest_uuid: uuid.UUID

    def __init__(self, _uuid: uuid.UUID) -> None:
        super().__init__()
        self.manifest_uuid = _uuid


class NoSuchManifestError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"no such manifest '{self.manifest_uuid}'"


class MalformedManifestError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"malformed manifest '{self.manifest_uuid}'"


class ReleaseManifest(pydantic.BaseModel):
    name: str
    base_release_name: str
    base_ref_org: str
    base_ref_repo: str
    base_ref: str

    patchsets: list[uuid.UUID] = pydantic.Field(default=[])

    creation_date: dt = pydantic.Field(default_factory=lambda: dt.now(datetime.UTC))
    release_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    release_git_uid: str = pydantic.Field(
        default_factory=lambda: "".join(choices(string.ascii_letters, k=6))  # noqa: S311
    )

    def contains_patchset(self, patchset: PatchSetBase) -> bool:
        """Check if the release manifest contains a given patch set."""
        return patchset.patchset_uuid in self.patchsets

    def add_patchset(self, patchset: PatchSetBase) -> bool:
        """
        Add a patch set to this release manifest.

        Returns a tuple containing:
        - `bool`, indicating whether the patch set was added or not.
        - `list[Patch]`, with the patches that were added to the release manifest.
        - `list[Patch]`, with the patches that were skipped and not added to the
                         release manifest.
        """
        if self.contains_patchset(patchset):
            return False

        self.patchsets.append(patchset.patchset_uuid)
        return True

    def gen_header(self) -> str:
        return f"""           name: {self.name}
   base release: {self.base_release_name}
base repository: {self.base_ref_org}/{self.base_ref_repo}
       base ref: {self.base_ref}
  creation date: {self.creation_date}
  manifest uuid: {self.release_uuid}"""
