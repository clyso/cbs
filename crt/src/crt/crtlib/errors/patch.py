# crt - errors - patch
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

from crt.crtlib.errors import CRTError


class PatchError(CRTError):
    def __init__(self, msg: str) -> None:
        super().__init__(msg)


class PatchExistsError(PatchError):
    def __init__(self, sha: str, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"sha'{sha}' uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"patch already exists: {self.msg}"


class NoSuchPatchError(PatchError):
    def __init__(self, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"patch not found: {self.msg}"


class MalformedPatchError(PatchError):
    def __init__(self, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"malformed patch: {self.msg}"
