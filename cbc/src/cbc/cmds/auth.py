# cbc - commands - auth
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

import errno
import logging
import sys

import click
from cbsdcore.auth.user import UserConfig

from cbc import CBC_DEFAULT_CONFIG_PATH
from cbc.auth import auth_ping, auth_whoami
from cbc.client import CBCConnectionError, CBCPermissionDeniedError
from cbc.cmds import logger as parent_logger
from cbc.cmds import pass_config, pass_logger, update_ctx

logger = parent_logger.getChild("auth")


@click.group("auth", help="auth related commands")
@update_ctx
def cmd_auth() -> None:
    pass


@cmd_auth.command("login", help="Log into a CBS service instance")
@click.argument("host", type=str, metavar="URL", required=True)
@update_ctx
@pass_logger
def cmd_auth_login(logger: logging.Logger, host: str) -> None:
    logger.debug(f"login to {host}")
    if not auth_ping(logger, host):
        click.echo(f"server at '{host}' not reachable", err=True)
        sys.exit(errno.EADDRNOTAVAIL)

    click.echo("please follow the URL to login")
    click.echo()
    click.echo(f"\t{host}/api/auth/login")
    click.echo()
    click.echo(f"Once logged in, copy the file to {CBC_DEFAULT_CONFIG_PATH}")
    pass


@cmd_auth.command("whoami", help="Checks user is logged in")
@update_ctx
@pass_logger
@pass_config
def cmd_auth_whoami(config: UserConfig, logger: logging.Logger) -> None:
    logger.debug(f"config: {config}")
    try:
        email, name = auth_whoami(logger, config)
        click.echo(f"email: {email}")
        click.echo(f" name: {name}")
    except CBCConnectionError as e:
        click.echo(f"connection error: {e}", err=True)
        sys.exit(errno.ECONNREFUSED)
    except CBCPermissionDeniedError as e:
        click.echo(f"permission denied: {e}", err=True)
        sys.exit(errno.EACCES)
    except Exception as e:
        click.echo(f"error obtaining whoami: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)
