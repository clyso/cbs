# CES library - errors
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


class CESError(Exception):
    msg: str | None

    def __init__(self, msg: str | None = None) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return "CES error" + (f": {self.msg}" if self.msg is not None else "")


class MalformedVersionError(CESError):
    @override
    def __str__(self) -> str:
        return "malformed version"


class NoSuchVersionError(CESError):
    @override
    def __str__(self) -> str:
        return "no such version"


class UnknownRepositoryError(CESError):
    repo: str

    def __init__(self, repo: str) -> None:
        super().__init__()
        self.repo = repo

    @override
    def __str__(self) -> str:
        return f"unknown repository: {self.repo}"
