# Ceph Release Tool - manifest stages
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
from pathlib import Path

from crtlib.errors.manifest import EmptyActiveStageError
from crtlib.errors.stages import (
    MissingStagePatchError,
    StageError,
    StagePatchesExistError,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.manifest import ManifestStage, ReleaseManifest
from crtlib.utils import split_version_into_paths

logger = parent_logger.getChild("stages")


def stage_commit(
    patches_repo_path: Path,
    manifest: ReleaseManifest,
    *,
    force: bool = False,
    stage_uuid: uuid.UUID | None = None,
) -> tuple[int, ManifestStage]:
    # propagate 'no stage' and 'no active stage' errors.
    commit_stage = (
        manifest.get_stage(stage_uuid)
        if force and stage_uuid
        else manifest.get_active_stage()
    )

    if not commit_stage.patches:
        logger.error("empty stage, don't commit")
        raise EmptyActiveStageError(uuid=manifest.release_uuid)

    tags_str = "-".join([f"{t}.{n}" for t, n in commit_stage.tags])
    version = f"{manifest.name}-{tags_str}"

    dst_paths = split_version_into_paths(version)
    if not dst_paths:
        msg = f"unable to obtain destination paths for '{version}'"
        logger.error(msg)
        raise StageError(msg=msg)

    logger.debug(f"destination paths '{version}': {dst_paths}")

    target_path = patches_repo_path.joinpath("ces").joinpath(next(reversed(dst_paths)))
    target_path.mkdir(parents=True, exist_ok=True)

    existing_patches = list(target_path.glob("*.patch"))
    if existing_patches and not force:
        msg = f"patches exist for version '{version}'"
        logger.error(msg)
        raise StagePatchesExistError(msg=msg)

    # drop any existing symlinks for this release stage
    # this is only iterated over if 'existing_patches' is not empty, which means we got
    # here because 'force' is set.
    for p in existing_patches:
        if not p.is_symlink():
            msg = f"patch '{p.name}' version '{version}' not a symlink!"
            logger.error(msg)
            raise StageError(msg=msg)
        p.unlink()

    patch_n = 0

    for stage in manifest.stages:
        logger.debug(
            f"handle patches for version '{version}' stage '{stage.stage_uuid}'"
        )

        for p in stage.patches:
            patch = p.contents

            patch_path = (
                patches_repo_path.joinpath("ceph")
                .joinpath("patches")
                .joinpath(f"{patch.entry_uuid}.patch")
            )
            if not patch_path.exists():
                msg = f"missing patch for uuid '{patch.entry_uuid}' version '{version}'"
                logger.error(msg)
                raise MissingStagePatchError(msg=msg)

            patch_n = patch_n + 1
            target_patch_name = f"{patch_n:04d}-{patch.canonical_title}.patch"
            target_patch_lnk = target_path.joinpath(target_patch_name)

            relative_to_root_path = patches_repo_path.relative_to(
                target_path, walk_up=True
            )
            patch_path_relative_to_root = patch_path.relative_to(patches_repo_path)
            relative_patch_path = relative_to_root_path.joinpath(
                patch_path_relative_to_root
            )

            logger.debug(f"symlink '{target_patch_lnk}' to '{relative_patch_path}'")
            target_patch_lnk.symlink_to(relative_patch_path)

        if stage_uuid and stage.stage_uuid == commit_stage.stage_uuid:
            logger.debug("stop applying further stages, if any")
            break

    commit_stage.committed = True

    return (patch_n, commit_stage)
