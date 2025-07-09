# crt - patch set
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

from crtlib.errors.patchset import PatchSetCheckError, PatchSetError
from crtlib.git_utils import (
    GitEmptyPatchDiffError,
    GitPatchDiffError,
    git_check_patches_diff,
)
from crtlib.logger import logger as parent_logger
from crtlib.models.patch import Patch
from crtlib.models.patchset import PatchSetBase
from crtlib.patch import PatchError, patch_import

logger = parent_logger.getChild("patchset")


def patchset_check_patches_diff(
    ceph_git_path: Path, patchset: PatchSetBase, patchset_branch: str, base_ref: str
) -> tuple[list[str], list[str]] | None:
    logger.debug(f"check patchset branch '{patchset_branch}' against '{base_ref}'")

    try:
        added, skipped = git_check_patches_diff(
            ceph_git_path, base_ref, patchset_branch, limit=patchset.get_base_sha
        )
    except GitEmptyPatchDiffError:
        logger.warning(
            f"empty patch diff between patchset '{patchset_branch}' and '{base_ref}'"
        )
        return None
    except GitPatchDiffError as e:
        msg = f"unable to check patchset '{patchset_branch}' against '{base_ref}': {e}"
        logger.error(msg)
        raise PatchSetCheckError(msg=msg) from None

    logger.debug(f"patchset '{patchset_branch}' add {added}")
    logger.debug(f"patchset '{patchset_branch}' drop {skipped}")

    if len(added) + len(skipped) != len(patchset.patches):
        msg = "missing patches from patchset diff"
        logger.error(msg)
        raise PatchSetCheckError(msg=msg)

    return (added, skipped)


def patchset_import_patches(
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patches: list[Patch],
    target_version: str,
) -> None:
    for patch in patches:
        try:
            patch_import(
                patches_repo_path,
                ceph_repo_path,
                patch.sha,
                target_version=target_version,
            )
        except PatchError as e:
            msg = f"unable to import patch sha '{patch.sha}': {e}"
            logger.error(msg)
            raise PatchSetError(msg=msg) from None

        logger.info(f"imported patch set's patch sha '{patch.sha}'")
    pass
