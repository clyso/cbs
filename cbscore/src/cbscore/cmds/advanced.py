# cbsbuild - commands - advanced commands
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

import asyncio
import errno
import sys

import click

from cbscore.cmds import Ctx, pass_ctx, with_config
from cbscore.cmds import logger as parent_logger
from cbscore.config import Config, VaultConfig
from cbscore.utils._migration import migrate_releases_v1
from cbscore.utils.secrets import SecretsVaultMgr
from cbscore.utils.vault import VaultError

logger = parent_logger.getChild("advanced")


@click.group(hidden=True)
def cmd_advanced() -> None:
    pass


@cmd_advanced.command(
    "migrate-releases-v1",
    help="Migrate components' and releases' build descriptors from v1",
)
@with_config
@pass_ctx
def cmd_advanced_migrate_releases_v1(
    ctx: Ctx,
    config: Config,
    # secrets_path: Path,
) -> None:
    if not ctx.vault_config_path or not ctx.vault_config_path.exists():
        logger.error("vault config path not provided or does not exist")
        sys.exit(errno.ENOENT)

    try:
        vault_config = VaultConfig.load(ctx.vault_config_path)
    except Exception as e:
        logger.error(f"unable to read vault config from '{ctx.vault_config_path}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        secrets = SecretsVaultMgr(config.secrets_path, vault_config)
    except VaultError as e:
        logger.error(f"error logging in to vault: {e}")
        sys.exit(errno.EACCES)

    asyncio.run(migrate_releases_v1(secrets))
