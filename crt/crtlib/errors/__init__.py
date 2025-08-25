# crt - errors
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

import errno
from typing import override


class CRTError(Exception):
    msg: str | None
    ec: int | None

    def __init__(self, msg: str | None = None, *, ec: int | None = None):
        super().__init__()
        self.msg = msg
        self.ec = ec

    @override
    def __str__(self) -> str:
        ec_name = (
            errno.errorcode[self.ec] if self.ec and self.ec in errno.errorcode else None
        )
        return (
            "CRT error"
            + (f" ({ec_name})" if ec_name else "")
            + (f": {self.msg}" if self.msg else "")
        )

    def with_maybe_msg(self, prefix: str) -> str:
        return prefix + f": {self.msg}" if self.msg else ""
