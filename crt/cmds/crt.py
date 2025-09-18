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


from pathlib import Path

import click
from crtlib.logger import logger_set_handler
from rich.logging import RichHandler
from rich.padding import Padding
from rich.table import Table

from cmds import patch, release, stages

from . import (
    Ctx,
    console,
    manifest,
    pass_ctx,
    patchset,
    set_debug_logging,
    set_verbose_logging,
)
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
    "-v",
    "--verbose",
    is_flag=True,
    default=False,
    required=False,
    help="Show verbose output.",
)
@click.option(
    "--github-token",
    type=str,
    metavar="TOKEN",
    envvar="CRT_GITHUB_TOKEN",
    required=False,
    help="Specify GitHub Token to use.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    envvar="CRT_PATCHES_REPO_PATH",
    required=True,
    help="Path to CES patches git repository.",
)
@pass_ctx
def cmd_crt(
    ctx: Ctx,
    debug: bool,
    verbose: bool,
    github_token: str | None,
    patches_repo_path: Path,
) -> None:
    if verbose:
        set_verbose_logging()

    if debug:
        set_debug_logging()

    rich_handler = RichHandler(rich_tracebacks=True, console=console)
    logger_set_handler(rich_handler)

    ctx.github_token = github_token
    ctx.patches_repo_path = patches_repo_path

    if debug or verbose:
        table = Table(show_header=False, show_lines=False, box=None)
        table.add_column(justify="right", style="cyan", no_wrap=True)
        table.add_column(justify="left", style="magenta", no_wrap=False)
        table.add_row("github token", ctx.github_token or "[not set]")
        table.add_row("patches repo", str(ctx.patches_repo_path))
        console.print(Padding(table, (1, 0, 1, 0)))


# release manifest commands
cmd_crt.add_command(manifest.cmd_manifest_new)
cmd_crt.add_command(manifest.cmd_manifest_from)
cmd_crt.add_command(manifest.cmd_manifest_list)
cmd_crt.add_command(manifest.cmd_manifest_info)
cmd_crt.add_command(manifest.cmd_manifest_add_patchset)
cmd_crt.add_command(manifest.cmd_manifest_validate)
cmd_crt.add_command(manifest.cmd_manifest_publish)
cmd_crt.add_command(manifest.cmd_manifest_release_notes)

# command groups
cmd_crt.add_command(manifest.cmd_manifest)
cmd_crt.add_command(patchset.cmd_patchset)
cmd_crt.add_command(patch.cmd_patch)
cmd_crt.add_command(stages.cmd_manifest_stage)
cmd_crt.add_command(release.cmd_release)
