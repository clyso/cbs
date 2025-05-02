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
from ceslib.errors import CESError
from ceslib.images.desc import get_image_desc
from ceslib.images.sync import sync_image
from ceslib.logger import log as root_logger
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError

ourdir = os.path.dirname(os.path.realpath(__file__))

log = root_logger.getChild("images-tool")


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
        log.info("perform dry run")
    if force:
        log.info("force sync")

    log.debug(f"desc: {desc}")

    try:
        secrets = SecretsVaultMgr(
            secrets_path,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit=vault_transit,
        )
    except VaultError as e:
        log.error(f"error initializing secrets: {e}")
        sys.exit(1)
    except Exception as e:
        log.error(f"unknown error: {e}")
        sys.exit(1)

    for image in desc.images:
        log.info(f"copying '{image.src}' to '{image.dst}")
        try:
            sync_image(image.src, image.dst, secrets, force=force, dry_run=dry_run)
        except CESError as e:
            log.error(f"error copying images: {e}")
            sys.exit(1)
        except Exception as e:
            log.error(f"unknown error: {e}")
            sys.exit(1)

        log.info(f"copied image from '{image.src}' to '{image.dst}'")
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
