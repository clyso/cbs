# CBC - CES Build Service Client
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

# pyright: reportAny=false

import logging
from pathlib import Path

import click
from cbsdcore.auth.user import UserConfig

from cbc import CBC_DEFAULT_CONFIG_PATH, logger
from cbc import set_debug_logging as cbc_set_debug_logging
from cbc.cmds import Ctx, pass_ctx
from cbc.cmds.auth import cmd_auth
from cbc.cmds.builds import cmd_build

_cbc_help_message = """CES Build Service Client

Interacts with a CBS service, allowing the user to perform various
build-related actions, such as listing existing builds, on-going builds,
and trigger new builds.

See subcommands' descriptions for more information.
"""


@click.group(help=_cbc_help_message)
@click.option(
    "-d",
    "--debug",
    is_flag=True,
    default=False,
    help="Enable debug logging",
)
@click.option(
    "-c",
    "--config",
    "config_path",
    type=click.Path(
        exists=True,
        file_okay=True,
        dir_okay=False,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=False,
    help="Specify cbs config JSON file",
)
@pass_ctx
def main(ctx: Ctx, debug: bool, config_path: Path | None) -> None:
    if debug:
        cbc_set_debug_logging()

    logging.getLogger("httpx").setLevel(logging.DEBUG if debug else logging.CRITICAL)

    logger.info(f"config path: {config_path}")
    user_config_path: Path = CBC_DEFAULT_CONFIG_PATH
    if config_path:
        user_config_path = config_path

    if user_config_path.exists() and user_config_path.is_file():
        ctx.config = UserConfig.load(user_config_path)


main.add_command(cmd_auth)
main.add_command(cmd_build)

if __name__ == "__main__":
    main()
