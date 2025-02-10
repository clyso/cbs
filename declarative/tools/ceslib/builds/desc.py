# CES library - build descriptor
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

from ceslib.builds.errors import InvalidBuildDescriptorError, NoSuchBuildDescriptorError


class BuildSignedOffBy(pydantic.BaseModel):
    user: str
    email: str


class BuildComponent(pydantic.BaseModel):
    name: str
    repo: str
    version: str


class BuildDescriptor(pydantic.BaseModel):
    version: str
    title: str
    signed_off_by: BuildSignedOffBy
    components: list[BuildComponent]

    @classmethod
    def read(cls, path: Path) -> BuildDescriptor:
        # propagate exceptions
        with path.open("r") as f:
            raw_json = f.read()

        try:
            return BuildDescriptor.model_validate_json(raw_json)
        except OSError as e:
            if e.errno == errno.ENOENT:
                raise NoSuchBuildDescriptorError(path)
            raise e
        except pydantic.ValidationError:
            raise InvalidBuildDescriptorError(path)
        except Exception as e:
            raise e

    def write(self, path: Path) -> None:
        # propagate exceptions
        with path.open("w") as f:
            _ = f.write(self.model_dump_json(indent=2))
