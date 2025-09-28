# CES library - CES builder, utils
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

from cbscore.builder import BuilderError, MissingScriptError
from cbscore.builder import logger as parent_logger
from cbscore.utils import CmdArgs, CommandError, async_run_cmd
from cbscore.utils.paths import (
    get_component_scripts_path,
    get_script_path,
)

logger = parent_logger.getChild("utils")


async def get_component_version(
    component_name: str, components_path: Path, repo_path: Path
) -> str:
    """
    Obtain a component's version.

    Version is obtained by running the component's provided 'get_version.*' script,
    and returning the obtained value.

    The `components_path` argument refers to the directory under which we can find the
    components supported (under which a `scripts/` directory is expected).

    Raises `MissingScriptError` if the version script is not found.
    """
    scripts_path = get_component_scripts_path(components_path, component_name)
    if not scripts_path:
        msg = (
            f"unable to find scripts path for component '{component_name}' "
            + f"under '{components_path}"
        )
        logger.error(msg)
        raise BuilderError(msg)

    version_script_path = get_script_path(scripts_path, "get_version.*")
    if not version_script_path:
        msg = f"unable to find 'get_version' script for component '{component_name}'"
        logger.error(msg)
        raise MissingScriptError("get_version", msg=msg)

    cmd: CmdArgs = [
        version_script_path.resolve().as_posix(),
    ]

    try:
        rc, stdout, stderr = await async_run_cmd(cmd, cwd=repo_path)
    except CommandError as e:
        msg = f"error running version script for '{component_name}': {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e
    except Exception as e:
        msg = f"unknown exception running version script for '{component_name}: {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e

    if rc != 0:
        msg = f"error running version script for '{component_name}': {stderr}"
        logger.error(msg)
        raise BuilderError(msg)

    return stdout.strip()
