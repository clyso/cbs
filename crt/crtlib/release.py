# crt - release
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

import pydantic

from crtlib.errors.release import NoSuchReleaseError, ReleaseError
from crtlib.models.release import Release


def load_release(patches_repo_path: Path, release_name: str) -> Release:
    """Load a release from disk."""
    rel_meta_path = patches_repo_path / "ceph" / "releases" / f"{release_name}.json"
    if not rel_meta_path.exists():
        raise NoSuchReleaseError(release_name)

    try:
        return Release.model_validate_json(rel_meta_path.read_text(encoding="utf-8"))
    except pydantic.ValidationError as e:
        raise ReleaseError(f"invalid release file '{rel_meta_path}': {e}") from e
    except Exception as e:
        raise ReleaseError(f"cannot read release file '{rel_meta_path}': {e}") from e


def store_release(patches_repo_path: Path, release: Release) -> None:
    """Store a release to disk."""
    rel_meta_path = patches_repo_path / "ceph" / "releases" / f"{release.name}.json"
    try:
        rel_meta_path.parent.mkdir(parents=True, exist_ok=True)
        _ = rel_meta_path.write_text(
            release.model_dump_json(indent=2), encoding="utf-8"
        )
    except Exception as e:
        raise ReleaseError(f"cannot write release file '{rel_meta_path}': {e}") from e


def release_exists(patches_repo_path: Path, release_name: str) -> bool:
    """Check if a release exists on disk."""
    rel_meta_path = patches_repo_path / "ceph" / "releases" / f"{release_name}.json"
    return rel_meta_path.exists()
