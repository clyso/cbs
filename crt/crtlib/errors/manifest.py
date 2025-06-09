# crt - errors - release manifest
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

import uuid
from typing import override

from crtlib.errors import CRTError


class ManifestError(CRTError):
    manifest_uuid: uuid.UUID

    def __init__(self, _uuid: uuid.UUID) -> None:
        super().__init__()
        self.manifest_uuid = _uuid


class NoSuchManifestError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"no such manifest '{self.manifest_uuid}'"


class MalformedManifestError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"malformed manifest '{self.manifest_uuid}'"
