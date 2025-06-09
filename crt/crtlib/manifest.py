# crt - release manifests
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

from crtlib.apply import apply_manifest
from crtlib.db import ReleasesDB
from crtlib.logger import logger as parent_logger
from crtlib.models.manifest import ReleaseManifest

logger = parent_logger.getChild("manifest")


def manifest_apply(
    db: ReleasesDB,
    manifest: ReleaseManifest,
    repo_path: Path,
    token: str,
    repo: str,
    push: bool,
) -> None:
    logger.debug(f"apply manifest '{manifest.release_uuid}' to repo '{repo}'")

    target_branch = f"{manifest.name}-{manifest.release_git_uid}"

    # propagate exceptions
    res, added, skipped = apply_manifest(
        db, manifest, repo_path, token, target_branch, no_cleanup=True
    )

    logger.debug(f"manifest '{manifest.release_uuid}' applied to '{target_branch}'")
    logger.debug(f"patches added: {len(added)}, skipped: {len(skipped)}")

    if not res:
        logger.debug(f"no patches added to '{target_branch}', skip pushing.")
        return

    pass
