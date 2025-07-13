# crt - models - common type discriminators
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


from typing import Annotated, Any, cast

import pydantic
from crtlib.models.common import ManifestPatchEntry, ManifestPatchSetEntryType
from crtlib.models.patch import PatchMeta
from crtlib.models.patchset import GitHubPullRequest


def _patchset_entry_discriminator(
    v: Any,  # pyright: ignore[reportExplicitAny, reportAny]
) -> str:
    if isinstance(v, ManifestPatchEntry):
        return v.entry_type
    elif isinstance(v, dict) and "entry_type" in v:
        return cast(str, v["entry_type"])

    raise ValueError()


class ManifestPatchEntryWrapper(pydantic.BaseModel):
    contents: Annotated[
        Annotated[
            GitHubPullRequest, pydantic.Tag(ManifestPatchSetEntryType.PATCHSET_GITHUB)
        ]
        | Annotated[PatchMeta, pydantic.Tag(ManifestPatchSetEntryType.SINGLE)],
        pydantic.Discriminator(_patchset_entry_discriminator),
    ]
