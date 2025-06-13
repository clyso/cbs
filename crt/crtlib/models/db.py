# crt - models - database models
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

import pydantic
from crtlib.models.manifest import ReleaseManifest


class DBLocalManifestWrapper(pydantic.BaseModel):
    orig_etag: str | None
    orig_hash: str | None

    manifest: ReleaseManifest


class DBManifestInfo(pydantic.BaseModel):
    orig_hash: str | None
    orig_etag: str | None
    remote: bool
