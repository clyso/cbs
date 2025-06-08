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

import logging
from pathlib import Path

import click
from crtlib.logger import logger

from cmds import Ctx, manifest, pass_ctx, patchset


@click.group()
@click.option(
    "-d",
    "--debug",
    is_flag=True,
    default=False,
    required=False,
)
@click.option(
    "--db",
    "db_path",
    type=click.Path(
        exists=False,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
        readable=True,
        writable=True,
        path_type=Path,
    ),
    metavar="DIR",
    required=False,
    help="Specify manifest database path.",
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
    db_path: Path | None,
    github_token: str | None,
) -> None:
    if debug:
        logger.setLevel(logging.DEBUG)

    if db_path:
        ctx.db_path = db_path
    ctx.db_path.mkdir(exist_ok=True)
    ctx.github_token = github_token

    logger.debug(f"releases db path: {ctx.db_path}")
    logger.debug(f"  manifests path: {ctx.db.manifests_path}")
    logger.debug(f" patch sets path: {ctx.db.patchsets_path}")
    logger.debug(f"    patches path: {ctx.db.patches_path}")
    logger.debug(f"has github token: {github_token is not None}")


cmd_crt.add_command(manifest.cmd_manifest)
cmd_crt.add_command(patchset.cmd_patchset)
