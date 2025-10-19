#!/usr/bin/env python3

# Handles CES images copy, sync, and signing
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
import logging
import os
import sys
from pathlib import Path

import click

from cbscore.errors import CESError
from cbscore.images.desc import get_image_desc
from cbscore.images.sync import sync_image
from cbscore.logger import logger as root_logger
from cbscore.utils.secrets import SecretsVaultMgr
from cbscore.utils.vault import VaultError

ourdir = os.path.dirname(os.path.realpath(__file__))

logger = root_logger.getChild("images-tool")


async def _sync(
    version: str,
    force: bool,
    dry_run: bool,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
    secrets_path: Path,
) -> None:
    try:
        desc = await get_image_desc(version)
    except CESError as e:
        click.echo(f"error: {e}")
        sys.exit(1)

    if dry_run:
        logger.info("perform dry run")
    if force:
        logger.info("force sync")

    logger.debug(f"desc: {desc}")

    try:
        secrets = SecretsVaultMgr(
            secrets_path,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit=vault_transit,
        )
    except VaultError:
        logger.exception("error initializing secrets")
        sys.exit(1)
    except Exception:
        logger.exception("unknown error")
        sys.exit(1)

    for image in desc.images:
        logger.info(f"copying '{image.src}' to '{image.dst}")
        try:
            sync_image(image.src, image.dst, secrets, force=force, dry_run=dry_run)
        except CESError:
            logger.exception("error copying images")
            sys.exit(1)
        except Exception:
            logger.exception("unknown error")
            sys.exit(1)

        logger.info(f"copied image from '{image.src}' to '{image.dst}'")
    pass


@click.group()
@click.option("-d", "--debug", envvar="CES_TOOL_DEBUG", is_flag=True)
def main(debug: bool) -> None:
    if debug:
        root_logger.setLevel(logging.DEBUG)
    pass


@main.command()
@click.argument("version", type=str)
@click.option("-f", "--force", is_flag=True, default=False)
@click.option("--dry-run", is_flag=True, default=False)
@click.option("--vault-addr", envvar="VAULT_ADDR", type=str, required=True)
@click.option("--vault-role-id", envvar="VAULT_ROLE_ID", type=str, required=True)
@click.option("--vault-secret-id", envvar="VAULT_SECRET_ID", type=str, required=True)
@click.option("--vault-transit", envvar="VAULT_TRANSIT", type=str, required=True)
@click.option(
    "--secrets",
    "secrets_path",
    type=click.Path(
        exists=True, file_okay=True, dir_okay=False, readable=True, path_type=Path
    ),
    required=True,
)
def sync(
    version: str,
    force: bool,
    dry_run: bool,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
    secrets_path: Path,
) -> None:
    asyncio.run(
        _sync(
            version,
            force,
            dry_run,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit,
            secrets_path,
        )
    )
    pass


@main.command()
def verify() -> None:
    pass


if __name__ == "__main__":
    main()
