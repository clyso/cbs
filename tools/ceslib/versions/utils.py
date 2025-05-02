# CES library - versions utils
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


import re

from ceslib.errors import MalformedVersionError


# obtain major version from supplied CES version.
# it can follow the CES format (ces-vXX.YY.mm.*) or any version starting
# with 'vXX.*', such as ceph upstream versions, and other versions.
def get_major_version(v: str) -> str:
    m = re.match(r"^((?:ces-.*v|v)?\d+\.\d+).*", v)
    if m is None:
        raise MalformedVersionError(v)
    return m.group(1)


# obtain minor version from supplied version.
# it can follow the CES format (ces-vXX.YY.mm.*) or any version starting
# with 'vXX.*', such as ceph upstream versions, and other versions.
def get_minor_version(v: str) -> str | None:
    m = re.match(r"^((?:ces-.*v|v)?\d+\.\d+\.\d+).*", v)
    if m is None:
        return None
    return m.group(1)


def normalize_version(v: str) -> str:
    m = re.match(r"((ces-.*v|v)?\d+\.\d+\..*)", v)
    if not m:
        raise MalformedVersionError(v)

    ver, prefix = m.groups()
    if not prefix:
        return f"v{ver}"
    return ver
