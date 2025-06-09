# crt - errors - patchset
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

from crtlib.errors import CRTError


class PatchSetError(CRTError):
    def __init__(self, msg: str | None = None) -> None:
        super().__init__(msg)

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("patch set error")


class NoSuchPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("patch set does not exists")


class MalformedPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("malformed patch set")


class PatchSetMismatchError(PatchSetError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("mismatch patch set type")


class PatchSetCheckError(PatchSetError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("patch set check error")


class EmptyPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("patch set is empty")


class PatchSetExistsError(PatchSetError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("patch set already exists")
