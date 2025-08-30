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


from crtlib.logger import logger as parent_logger

logger = parent_logger.getChild("stages")

# keep this as reference, because we will need it later when publishing a release.
#
# for p in stage.patches:
#     patch = p.contents
#
#     patch_path = (
#         patches_repo_path.joinpath("ceph")
#         .joinpath("patches")
#         .joinpath(f"{patch.entry_uuid}.patch")
#     )
#     if not patch_path.exists():
#         msg = f"missing patch for uuid '{patch.entry_uuid}' version '{version}'"
#         logger.error(msg)
#         raise MissingStagePatchError(msg=msg)
#
#     patch_n = patch_n + 1
#     target_patch_name = f"{patch_n:04d}-{patch.canonical_title}.patch"
#     target_patch_lnk = target_path.joinpath(target_patch_name)
#
#     relative_to_root_path = patches_repo_path.relative_to(
#         target_path, walk_up=True
#     )
#     patch_path_relative_to_root = patch_path.relative_to(patches_repo_path)
#     relative_patch_path = relative_to_root_path.joinpath(
#         patch_path_relative_to_root
#     )
#
#     logger.debug(f"symlink '{target_patch_lnk}' to '{relative_patch_path}'")
#     target_patch_lnk.symlink_to(relative_patch_path)
