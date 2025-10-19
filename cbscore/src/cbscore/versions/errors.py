# CES library - builds errors
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

from pathlib import Path
from typing import override

from cbscore.errors import CESError


class VersionError(CESError):
    @override
    def __str__(self) -> str:
        return "Version Error" + (f": {self.msg}" if self.msg else "")


class InvalidVersionDescriptorError(VersionError):
    path: Path | None

    def __init__(self, path: Path | None = None) -> None:
        super().__init__()
        self.path = path

    @override
    def __str__(self) -> str:
        return "invalid build descriptor" + (
            f" at '{self.path}'" if self.path is not None else ""
        )


class NoSuchVersionDescriptorError(VersionError):
    path: Path | None

    def __init__(self, path: Path | None = None) -> None:
        super().__init__()
        self.path = path

    @override
    def __str__(self) -> str:
        return "no such build descriptor" + (
            f" at '{self.path}'" if self.path is not None else ""
        )
