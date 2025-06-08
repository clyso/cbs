# crt - Ceph Release Tool library
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


class CRTError(Exception):
    msg: str | None

    def __init__(self, msg: str | None = None):
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return "CRT error" + (f": {self.msg}" if self.msg else "")
