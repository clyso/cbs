# crt - db
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

import enum
import os
import uuid
from pathlib import Path

from crtlib.logger import logger as parent_logger

logger = parent_logger.getChild("db")

S3_DB_BUCKET = "ces-packages"
S3_DB_PATH = "releases/db"
S3_STATE_PATH = f"{S3_DB_PATH}/state.json"

CRT_CONTENT_TYPE_PREFIX = "application/vnd.clyso.crt"


class S3ContentType(enum.StrEnum):
    STATE = f"{CRT_CONTENT_TYPE_PREFIX}.state+json"
    MANIFEST = f"{CRT_CONTENT_TYPE_PREFIX}.manifest+json"
    UUID = f"{CRT_CONTENT_TYPE_PREFIX}.uuid+text"
    PATCHSET = f"{CRT_CONTENT_TYPE_PREFIX}.patchset+json"


DBLoc = Path | str


def _to_loc(p: DBLoc) -> str:
    if isinstance(p, Path):
        return p.as_posix()
    return p


def get_manifests_loc(root: DBLoc) -> str:
    return os.path.join(_to_loc(root), "manifests")


def get_patchsets_loc(root: DBLoc) -> str:
    return os.path.join(_to_loc(root), "patchsets")


def get_gh_prs_loc(root: DBLoc) -> str:
    return os.path.join(get_patchsets_loc(root), "gh")


def get_patches_loc(root: DBLoc) -> str:
    return os.path.join(_to_loc(root), "patches")


def get_patches_by_uuid_loc(root: DBLoc) -> str:
    return os.path.join(get_patches_loc(root), "by_uuid")


def get_patches_by_sha_loc(root: DBLoc) -> str:
    return os.path.join(get_patches_loc(root), "by_sha")


def manifest_loc(root: DBLoc, _uuid: uuid.UUID) -> str:
    return os.path.join(get_manifests_loc(root), f"{_uuid}.json")
