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
from crtlib.git_utils import SHA
from crtlib.models.common import AuthorData


class ManifestError(CRTError):
    manifest_uuid: uuid.UUID

    def __init__(self, _uuid: uuid.UUID, msg: str | None = None) -> None:
        super().__init__(msg)
        self.manifest_uuid = _uuid

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg(f"manifest error on '{self.manifest_uuid}'")


class NoSuchManifestError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"no such manifest '{self.manifest_uuid}'"


class ManifestExistsError(ManifestError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg(
            f"manifest uuid '{self.manifest_uuid}' already exists"
        )


class MalformedManifestError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"malformed manifest '{self.manifest_uuid}'"


class NoActiveManifestStageError(ManifestError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg(
            f"no active stage on manifest '{self.manifest_uuid}'"
        )


class ActiveManifestStageFoundError(ManifestError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg(
            f"active stage found on manifest '{self.manifest_uuid}'"
        )


class MismatchStageAuthorError(ManifestError):
    stage_author: AuthorData
    other_author: AuthorData

    def __init__(
        self, _uuid: uuid.UUID, stage_author: AuthorData, other_author: AuthorData
    ) -> None:
        super().__init__(_uuid)
        self.stage_author = stage_author
        self.other_author = other_author

    @override
    def __str__(self) -> str:
        return (
            "mismatched stage author:\n"
            + f"  expected: {self.stage_author}\n"
            + f"     found: {self.other_author}"
        )


class EmptyActiveStageError(ManifestError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg(
            f"no patch sets on active stage for manifest '{self.manifest_uuid}'"
        )


class NoStageError(ManifestError):
    @override
    def __str__(self) -> str:
        return f"no stage available for manifest '{self.manifest_uuid}'"


class ManifestCorruptedStageError(ManifestError):
    expected: SHA
    found: SHA

    def __init__(self, _uuid: uuid.UUID, expected: SHA, found: SHA) -> None:
        super().__init__(_uuid)
        self.expected = expected
        self.found = found

    @override
    def __str__(self) -> str:
        return (
            f"corrupted stage on manifest '{self.manifest_uuid}:\n"
            + f"expected hash: {self.expected}\n"
            + f"   found hash: {self.found}"
        )


class ManifestCorruptedError(ManifestError):
    expected: SHA
    found: SHA

    def __init__(self, _uuid: uuid.UUID, expected: SHA, found: SHA) -> None:
        super().__init__(_uuid)
        self.expected = expected
        self.found = found

    @override
    def __str__(self) -> str:
        return (
            f"corrupted manifest '{self.manifest_uuid}:\n"
            + f"expected hash: {self.expected}\n"
            + f"   found hash: {self.found}"
        )
