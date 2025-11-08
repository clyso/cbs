#!/usr/bin/env python3

# Builds a declarative version, using a container
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

import logging
from pathlib import Path

import click

from cbscore.cmds import (
    Ctx,
    advanced,
    builds,
    config,
    local,
    pass_ctx,
    set_log_level,
    versions,
)
from cbscore.cmds import logger as parent_logger

logger = parent_logger.getChild("main")


@click.group()
@click.option(
    "-d", "--debug", help="Enable debug output", is_flag=True, envvar="CBS_DEBUG"
)
@click.option(
    "-c",
    "--config",
    "config_path",
    help="Path to configuration file.",
    type=click.Path(
        exists=False,
        dir_okay=False,
        file_okay=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    default="cbs-build.config.yaml",
)
@pass_ctx
def cmd_main(ctx: Ctx, debug: bool, config_path: Path) -> None:
    if debug:
        set_log_level(logging.DEBUG)

    ctx.config_path = config_path


cmd_main.add_command(builds.cmd_build)
cmd_main.add_command(builds.cmd_runner_grp)
cmd_main.add_command(versions.cmd_versions_grp)
cmd_main.add_command(config.cmd_config)
cmd_main.add_command(advanced.cmd_advanced)
cmd_main.add_command(local.cmd_local_grp)


if __name__ == "__main__":
    cmd_main()
