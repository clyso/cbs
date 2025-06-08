# crt - patch
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
from crtlib.git import SHA


class PatchError(Exception):
    msg: str

    def __init__(self, msg: str) -> None:
        super().__init__()
        self.msg = msg


class PatchExistsError(PatchError):
    def __init__(self, sha: str, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"sha'{sha}' uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"patch already exists: {self.msg}"


class NoSuchPatchError(PatchError):
    def __init__(self, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"patch not found: {self.msg}"


class MalformedPatchError(PatchError):
    def __init__(self, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"malformed patch: {self.msg}"


class AuthorData(pydantic.BaseModel):
    """Represents an author."""

    user: str
    email: str


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
