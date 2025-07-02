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

import errno
import sys
from pathlib import Path

import click
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError

from cmds import db, patch

from . import Ctx, manifest, pass_ctx, patchset, perror, pwarn, set_debug_logging
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
    required=True,
    default=Path.cwd().joinpath(".releases"),
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
@click.option(
    "--vault-addr",
    type=str,
    metavar="ADDRESS",
    envvar="VAULT_ADDR",
    required=True,
    default="vault.clyso.cloud",
    help="Specify CES vault's address.",
)
@click.option(
    "--vault-role-id",
    type=str,
    metavar="ROLE-ID",
    envvar="VAULT_ROLE_ID",
    required=True,
    help="Specify CES vault's role ID.",
)
@click.option(
    "--vault-secret-id",
    type=str,
    metavar="SECRET-ID",
    envvar="VAULT_SECRET_ID",
    required=True,
    help="Specify CES vault's secret ID.",
)
@click.option(
    "--secrets-path",
    type=click.Path(
        exists=True,
        file_okay=True,
        dir_okay=False,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    envvar="CES_SECRETS_PATH",
    required=True,
    help="Path to CES secrets JSON file.",
)
@pass_ctx
def cmd_crt(
    ctx: Ctx,
    debug: bool,
    db_path: Path,
    github_token: str | None,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    secrets_path: Path,
) -> None:
    if debug:
        set_debug_logging()

    db_path.mkdir(exist_ok=True)
    ctx.github_token = github_token

    try:
        secrets = SecretsVaultMgr(
            secrets_path, vault_addr, vault_role_id, vault_secret_id
        )
    except VaultError as e:
        perror(f"unable to start vault secrets engine: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    ctx.init(db_path, secrets)

    logger.debug(f"releases db path: {ctx.db_path}")
    logger.debug(f"has github token: {github_token is not None}")
    logger.debug(f"      vault addr: {vault_addr}")
    logger.debug(f"   vault role id: {vault_role_id}")

    if not ctx.db.is_synced:
        pwarn(
            "S3 database not synced, please run "
            + "'[bold bright_magenta]db sync[/bold bright_magenta]'"
        )


cmd_crt.add_command(db.cmd_db)
cmd_crt.add_command(manifest.cmd_manifest)
cmd_crt.add_command(patchset.cmd_patchset)
cmd_crt.add_command(patch.cmd_patch)
