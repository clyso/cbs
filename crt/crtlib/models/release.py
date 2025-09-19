# crt - models - release
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

import datetime
from datetime import datetime as dt

import pydantic

from . import logger as parent_logger

logger = parent_logger.getChild("release")


class Release(pydantic.BaseModel):
    name: str
    creation_date: dt = pydantic.Field(default_factory=lambda: dt.now(datetime.UTC))
    is_published: bool = pydantic.Field(default=False)

    base_release_name: str
    base_release_ref: str
    base_repo: str

    release_repo: str
    release_base_branch: str
    release_base_tag: str
    release_branch: str
