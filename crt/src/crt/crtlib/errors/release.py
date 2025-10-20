# crt - errors - release
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

from crt.crtlib.errors import CRTError


class ReleaseError(CRTError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("release error")


class NoSuchReleaseError(ReleaseError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("no such release")
