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

from crt.crtlib.config import CrtStoreConfig, resolve_channel
from crt.crtlib.errors.release import NoSuchReleaseError, ReleaseError
from crt.crtlib.git_utils import (
    GitCreateHeadExistsError,
    git_branch_from,
    git_checkout_ref,
)
from crt.crtlib.models.release import Release
from crt.crtlib.paths import release_branch_name, release_path


def load_release(
    patches_repo_path: Path, ns: str, channel: str, release_name: str
) -> Release:
    """Load a release from disk."""
    rel_meta_path = release_path(patches_repo_path, ns, channel, release_name)
    if not rel_meta_path.exists():
        raise NoSuchReleaseError(release_name)

    try:
        return Release.model_validate_json(rel_meta_path.read_text(encoding="utf-8"))
    except pydantic.ValidationError as e:
        raise ReleaseError(f"invalid release file '{rel_meta_path}': {e}") from e
    except Exception as e:
        raise ReleaseError(f"cannot read release file '{rel_meta_path}': {e}") from e


def store_release(
    patches_repo_path: Path, ns: str, channel: str, release: Release
) -> None:
    """Store a release to disk."""
    rel_meta_path = release_path(patches_repo_path, ns, channel, release.name)
    try:
        rel_meta_path.parent.mkdir(parents=True, exist_ok=True)
        _ = rel_meta_path.write_text(
            release.model_dump_json(indent=2), encoding="utf-8"
        )
    except Exception as e:
        raise ReleaseError(f"cannot write release file '{rel_meta_path}': {e}") from e


def release_exists(
    patches_repo_path: Path, ns: str, channel: str, release_name: str
) -> bool:
    """Check if a release exists on disk."""
    rel_meta_path = release_path(patches_repo_path, ns, channel, release_name)
    return rel_meta_path.exists()


def resolve_and_load_release(
    patches_repo_path: Path, config: CrtStoreConfig, release_name: str
) -> tuple[str, str, Release]:
    """Load a release, resolving ns/channel from the release name."""
    ns, channel, _ = resolve_channel(config, release_name)
    return (ns, channel, load_release(patches_repo_path, ns, channel, release_name))


def create_release_branch(
    store_repo_path: Path, ns: str, release_name: str, src_ref: str = "main"
) -> str:
    """Create a release branch in the CRT store repository."""
    branch = release_branch_name(ns, release_name)
    try:
        git_branch_from(store_repo_path, src_ref, branch)
        git_checkout_ref(store_repo_path, branch)
    except GitCreateHeadExistsError:
        git_checkout_ref(store_repo_path, branch)
    except Exception as e:
        raise ReleaseError(f"cannot create release branch '{branch}': {e}") from e
    return branch
