# crt - models - patchset
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


from datetime import datetime as dt

import pydantic
from crtlib.models.common import AuthorData


class Version(pydantic.BaseModel):
    pass


class PatchSetIndex(pydantic.BaseModel):
    """Index a Patch Set."""

    author: AuthorData
    creation_date: dt
    title: str
    desc: str | None
    target_version: Version
