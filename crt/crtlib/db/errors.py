# crt - db - s3
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

from crtlib.errors import CRTError


class DBError(CRTError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("db error")


class S3DBError(DBError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("s3 db error")


class S3DBCredsError(S3DBError):
    def __init__(self, msg: str) -> None:
        super().__init__(msg)


class S3DBExistingManifestError(S3DBError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("manifest exists")


class S3DBConflictingManifestError(S3DBError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("conflicting manifest")
