# CBS - auth library
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

from cbslib.logger import log as parent_logger
from ceslib.errors import CESError


class AuthError(CESError):
    """An authentication related error occurred."""

    @override
    def __str__(self) -> str:
        return "Auth Error" + (f": {self.msg}" if self.msg else "")


class AuthNoSuchUserError(AuthError):
    """An authentication user is missing."""

    def __init__(self, user: str) -> None:
        super().__init__(f"no such user '{user}'")


log = parent_logger.getChild("auth")
