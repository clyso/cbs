# CES library - images errors
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

from typing import override

from cbscore.errors import CESError


class ImageDescriptorError(CESError):
    def __init__(self, msg: str | None = None) -> None:
        super().__init__(
            "image descriptor error" + (f": {msg}" if msg is not None else "")
        )


class SkopeoError(CESError):
    @override
    def __str__(self) -> str:
        return "skopeo error"


class AuthError(CESError):
    msg: str | None

    def __init__(self, msg: str | None) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return "authentication error" + ("" if self.msg is None else f": {self.msg}")


class MissingTagError(CESError):
    tag: str | None
    for_what: str

    def __init__(self, *, tag: str | None = None, for_what: str) -> None:
        super().__init__()
        self.tag = tag
        self.for_what = for_what

    @override
    def __str__(self) -> str:
        return (
            "missing tag "
            + (f"'{self.tag}' " if self.tag is not None else "")
            + f"for '{self.for_what}'"
        )


class ImageNotFoundError(SkopeoError):
    @override
    def __str__(self) -> str:
        return "image not found" + f": {self.msg}" if self.msg else ""
