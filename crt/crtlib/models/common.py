# crt - models - common models
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

from __future__ import annotations

import abc
import enum
import re
import uuid

import pydantic


class AuthorData(pydantic.BaseModel):
    """Represents an author."""

    user: str
    email: str


class ManifestPatchSetEntryType(enum.StrEnum):
    PATCHSET_VANILLA = "vanilla"
    PATCHSET_GITHUB = "gh"
    PATCHSET_CUSTOM = "custom"
    SINGLE = "single"


class ManifestPatchEntry(pydantic.BaseModel, abc.ABC):  # pyright: ignore[reportUnsafeMultipleInheritance]
    entry_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())

    @pydantic.computed_field
    @property
    def entry_type(self) -> ManifestPatchSetEntryType:
        return self._get_entry_type()

    @abc.abstractmethod
    def _get_entry_type(self) -> ManifestPatchSetEntryType:
        pass

    @property
    def canonical_title(self) -> str:
        return self._get_canonical_title()

    @abc.abstractmethod
    def _get_canonical_title(self) -> str:
        pass


def patch_canonical_title(orig: str) -> str:
    r1 = re.compile(r"[\s:/\]\[\(\)]")
    r2 = re.compile(r"['\",.+\<>~^$@!?%&=;`]")
    return r2.sub(r"", r1.sub(r"-", orig.lower()))
