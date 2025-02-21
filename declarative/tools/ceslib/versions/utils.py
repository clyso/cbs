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
# must be in the format 'ces-vXX.YY[.mm].*'
def get_major_version(v: str) -> str:
    m = re.match(r"^(ces-v\d{2}\.\d{2}).*", v)
    if m is None:
        raise MalformedVersionError(v)
    return m.group(1)


# obtain minor version from supplied CES version.
# must be in the format 'ces-vXX.YY.mm.*'
def get_minor_version(v: str) -> str | None:
    m = re.match(r"^(ces-v\d{2}\.\d{2}\.\d+).*", v)
    if m is None:
        return None
    return m.group(1)
