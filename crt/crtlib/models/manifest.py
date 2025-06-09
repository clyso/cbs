# crt - models - manifest
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

import pydantic
from crtlib.models.patchset import PatchSetBase


class ReleaseManifest(pydantic.BaseModel):
    name: str
    base_release_name: str
    base_ref_org: str
    base_ref_repo: str
    base_ref: str
    dst_repo: str

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

    def gen_header(self) -> list[tuple[str, str]]:
        return [
            ("name", self.name),
            ("base release", self.base_release_name),
            ("base repository", f"{self.base_ref_org}/{self.base_ref_repo}"),
            ("base ref", self.base_ref),
            ("dest repository", self.dst_repo),
            ("creation date", str(self.creation_date)),
            ("manifest uuid", str(self.release_uuid)),
        ]
