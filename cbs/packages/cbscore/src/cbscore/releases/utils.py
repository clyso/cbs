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


from cbscore.core.component import CoreComponentLoc
from cbscore.releases import ReleaseError
from cbscore.releases import logger as parent_logger
from cbscore.utils import CmdArgs, CommandError, async_run_cmd

logger = parent_logger.getChild("utils")


async def get_component_release_rpm(
    component_loc: CoreComponentLoc,
    el_version: int,
) -> str | None:
    component = component_loc.comp
    if not component.build.rpm:
        msg = f"component '{component.name}' has no rpm build configuration"
        logger.error(msg)
        raise ReleaseError(msg)

    release_rpm_script = component_loc.path / component.build.rpm.release_rpm
    if not release_rpm_script.exists():
        logger.warning(
            f"unable to find component release RPM for '{component.name}': "
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
        msg = f"error running release RPM script for '{component.name}': {e}"
        logger.exception(msg)
        raise ReleaseError(msg) from e
    except Exception as e:
        msg = f"unknown error running release RPM script for '{component.name}': {e}"
        logger.exception(msg)
        raise ReleaseError(msg) from e

    if rc != 0:
        msg = f"error running release RPM script for '{component.name}': {stderr}"
        logger.exception(msg)
        raise ReleaseError(msg)

    return stdout.strip()
