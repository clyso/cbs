# CES library - paths utilities
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

import stat
from pathlib import Path

from ceslib.utils import logger as parent_logger

logger = parent_logger.getChild("paths")


def get_component_scripts_path(
    components_path: Path, component_name: str
) -> Path | None:
    comp_path = components_path.joinpath(component_name)
    if not comp_path.exists():
        logger.warning(
            f"component path for '{component_name}' "
            + f"not found in '{components_path}'"
        )
        return None

    comp_scripts_path = comp_path.joinpath("scripts")
    if not comp_scripts_path.exists():
        logger.warning(
            f"component scripts path for '{component_name}' "
            + f"not found in '{comp_path}'"
        )
        return None

    return comp_scripts_path


def get_script_path(scripts_path: Path, glob: str) -> Path | None:
    candidates = list(scripts_path.glob(glob))
    if len(candidates) != 1:
        logger.error(
            f"found '{len(candidates)}' candidate build scripts in "
            + f"'{scripts_path}' for glob '{glob}', needs 1"
        )
        return None

    script_path = candidates[0]
    if not script_path.is_file() or not script_path.stat().st_mode & stat.S_IXUSR:
        logger.error(f"script at '{script_path}' either not a file or not executable")
        return None
    return script_path
