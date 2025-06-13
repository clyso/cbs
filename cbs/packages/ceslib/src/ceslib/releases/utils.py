# CES library - CES releases
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

from ceslib.releases import ReleaseError
from ceslib.releases import log as parent_logger
from ceslib.utils import CmdArgs, CommandError, async_run_cmd
from ceslib.utils.paths import get_component_scripts_path, get_script_path

log = parent_logger.getChild("utils")


async def get_component_release_rpm(
    components_path: Path,
    component_name: str,
    el_version: int,
) -> str | None:
    scripts_path = get_component_scripts_path(components_path, component_name)
    if not scripts_path:
        log.warning(
            f"unable to find component release RPM for '{component_name}': "
            + f"no scripts path at '{components_path}"
        )
        return None

    release_rpm_script = get_script_path(scripts_path, "get_release_rpm.*")
    if not release_rpm_script:
        log.warning(
            f"unable to find component release RPM for '{component_name}': "
            + "no script available"
        )
        return None

    cmd: CmdArgs = [
        release_rpm_script.resolve().as_posix(),
        str(el_version),
    ]

    try:
        rc, stdout, stderr = await async_run_cmd(cmd)
    except CommandError as e:
        msg = f"error running release RPM script for '{component_name}': {e}"
        log.exception(msg)
        raise ReleaseError(msg) from e
    except Exception as e:
        msg = f"unknown error running release RPM script for '{component_name}': {e}"
        log.exception(msg)
        raise ReleaseError(msg) from e

    if rc != 0:
        msg = f"error running release RPM script for '{component_name}': {stderr}"
        log.exception(msg)
        raise ReleaseError(msg)

    return stdout.strip()
