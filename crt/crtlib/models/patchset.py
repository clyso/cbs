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
import uuid
from datetime import datetime as dt
from typing import Annotated, Any

import pydantic
from crtlib.errors.patchset import EmptyPatchSetError
from crtlib.git import SHA
from crtlib.models.patch import AuthorData, Patch


class PatchSetBase(pydantic.BaseModel, abc.ABC):  # pyright: ignore[reportUnsafeMultipleInheritance]
    """Represents a set of related patches."""

    author: AuthorData
    creation_date: dt
    title: str
    related_to: list[str]
    patches: list[Patch]

    patchset_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())

    @property
    def get_base_sha(self) -> SHA:
        if not self.patches:
            raise EmptyPatchSetError(str(self.patchset_uuid))

        first_patch = next(iter(self.patches))
        return first_patch.parent


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
