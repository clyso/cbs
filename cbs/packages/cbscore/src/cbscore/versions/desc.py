# CES library - version descriptor
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

import errno
from pathlib import Path

import pydantic
from cbscore.versions.errors import (
    InvalidVersionDescriptorError,
    NoSuchVersionDescriptorError,
)


class VersionSignedOffBy(pydantic.BaseModel):
    user: str
    email: str


class VersionImage(pydantic.BaseModel):
    registry: str
    name: str
    tag: str


class VersionComponent(pydantic.BaseModel):
    name: str
    repo: str
    ref: str


class VersionDescriptor(pydantic.BaseModel):
    version: str
    title: str
    signed_off_by: VersionSignedOffBy
    image: VersionImage
    components: list[VersionComponent]
    distro: str
    el_version: int

    @classmethod
    def read(cls, path: Path) -> VersionDescriptor:
        # propagate exceptions
        with path.open("r") as f:
            raw_json = f.read()

        try:
            return VersionDescriptor.model_validate_json(raw_json)
        except OSError as e:
            if e.errno == errno.ENOENT:
                raise NoSuchVersionDescriptorError(path) from None
            raise e  # noqa: TRY201
        except pydantic.ValidationError:
            raise InvalidVersionDescriptorError(path) from None
        except Exception as e:
            raise e  # noqa: TRY201

    def write(self, path: Path) -> None:
        # propagate exceptions
        with path.open("w") as f:
            _ = f.write(self.model_dump_json(indent=2))
