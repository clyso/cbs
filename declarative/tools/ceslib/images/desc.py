# CES library - images descriptors
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

from pathlib import Path
import re
import pydantic

from ceslib.images import log as parent_logger
from ceslib.images.errors import (
    DescriptorError,
    MalformedVersionError,
    NoSuchVersionError,
)
from ceslib.utils.git import get_git_repo_root

log = parent_logger.getChild("descriptors")


class ImageLocations(pydantic.BaseModel):
    src: str
    dst: str


class ImageDescriptor(pydantic.BaseModel):
    releases: list[str]
    images: list[ImageLocations]


def get_version_desc(version: str) -> ImageDescriptor:
    m = re.match(r"(\d+\.\d+\.\d+).*", version)
    if m is None:
        raise MalformedVersionError()

    candidates: list[Path] = []

    def _file_matches(f: str) -> bool:
        return f.startswith(f"ces-{m[1]}") and f.endswith(".json")

    def _gen_candidates(base_path: Path, files: list[str]) -> list[Path]:
        return [base_path.joinpath(f) for f in files if _file_matches(f)]

    desc_path = get_git_repo_root().joinpath("desc")
    if not desc_path.exists():
        log.error(f"descriptor directory not found at '{desc_path}'")
        raise NoSuchVersionError()

    for cur_path, dirs, file_lst in desc_path.walk(top_down=True):
        log.debug(f"path: {cur_path}, dirs: {dirs}, files: {file_lst}")
        candidates.extend(_gen_candidates(cur_path, file_lst))

    log.debug(f"candidates: {candidates}")

    ces_version = f"ces-v{version}"

    desc: ImageDescriptor | None = None
    found_at: Path | None = None
    for candidate in candidates:
        try:
            desc_raw = candidate.read_text()
            desc = ImageDescriptor.model_validate_json(desc_raw)
        except Exception as e:
            log.debug(f"error loading desc file: {e}")
            raise e

        if ces_version in desc.releases:
            if found_at is not None:
                log.error(
                    f"error: potential conflict for version {ces_version} "
                    + f"between {found_at} and {candidate}"
                )
                raise DescriptorError()
            found_at = candidate
            desc = desc
            log.debug(f"found candidate at {found_at}")

    if found_at is not None:
        assert desc is not None
        return desc

    raise NoSuchVersionError()
