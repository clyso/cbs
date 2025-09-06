# Ceph Release Tool - root command
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


import click

from cmds import patch, stages

from . import Ctx, manifest, pass_ctx, patchset, set_debug_logging
from . import logger as parent_logger

logger = parent_logger.getChild("crt")


@click.group()
@click.option(
    "-d",
    "--debug",
    is_flag=True,
    default=False,
    required=False,
    help="Show debug output.",
)
@click.option(
    "--github-token",
    type=str,
    metavar="TOKEN",
    envvar="GITHUB_TOKEN",
    required=False,
    help="Specify GitHub Token to use.",
)
@pass_ctx
def cmd_crt(
    ctx: Ctx,
    debug: bool,
    github_token: str | None,
) -> None:
    if debug:
        set_debug_logging()

    ctx.github_token = github_token
    logger.debug(f"has github token: {github_token is not None}")


# release manifest commands
cmd_crt.add_command(manifest.cmd_manifest_new)
cmd_crt.add_command(manifest.cmd_manifest_from)
cmd_crt.add_command(manifest.cmd_manifest_list)
cmd_crt.add_command(manifest.cmd_manifest_info)
cmd_crt.add_command(manifest.cmd_manifest_add_patchset)
cmd_crt.add_command(manifest.cmd_manifest_validate)
cmd_crt.add_command(manifest.cmd_manifest_release_notes)

# command groups
cmd_crt.add_command(manifest.cmd_manifest)
cmd_crt.add_command(patchset.cmd_patchset)
cmd_crt.add_command(patch.cmd_patch)
cmd_crt.add_command(stages.cmd_manifest_stage)
