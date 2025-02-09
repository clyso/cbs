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

import logging
import os
import sys

import click
from ceslib.errors import CESError
from ceslib.logging import log as root_logger
from ceslib.images.auth import AuthAndSignInfo
from ceslib.images.desc import get_version_desc
from ceslib.images.errors import AuthError
from ceslib.images.sync import sync_image

ourdir = os.path.dirname(os.path.realpath(__file__))

log = root_logger.getChild("images-tool")


@click.group()
@click.option("-d", "--debug", is_flag=True)
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
def sync(
    version: str,
    force: bool,
    dry_run: bool,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
) -> None:
    try:
        desc = get_version_desc(version)
    except CESError as e:
        click.echo(f"error: {e}")
        sys.exit(1)

    if dry_run:
        log.info("perform dry run")
    if force:
        log.info("force sync")

    log.debug(f"desc: {desc}")

    try:
        auth_info = AuthAndSignInfo(
            vault_addr, vault_role_id, vault_secret_id, vault_transit
        )
    except AuthError as e:
        log.error(f"authentication error: {e}")
        sys.exit(1)
    except Exception as e:
        log.error(f"unknown error: {e}")
        sys.exit(1)

    for image in desc.images:
        log.info(f"copying '{image.src}' to '{image.dst}")
        try:
            sync_image(image.src, image.dst, auth_info, force=force, dry_run=dry_run)
        except CESError as e:
            log.error(f"error copying images: {e}")
            sys.exit(1)
        except Exception as e:
            log.error(f"unknown error: {e}")
            sys.exit(1)

        log.info(f"copied image from '{image.src}' to '{image.dst}'")


@main.command()
def verify() -> None:
    pass


if __name__ == "__main__":
    main()
