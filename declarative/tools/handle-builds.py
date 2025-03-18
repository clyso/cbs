#!/usr/bin/env python3

# Builds declarative versions added to the repository
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
import logging
import re
import sys
from pathlib import Path

import click
from ceslib.errors import CESError, MalformedVersionError, NoSuchVersionError
from ceslib.images.desc import ImageDescriptor, get_image_desc
from ceslib.images.sync import sync_image
from ceslib.logging import log as root_logger
from ceslib.utils.git import GitError, get_git_modified_paths
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError
from ceslib.versions.desc import VersionDescriptor

log = root_logger.getChild("handle-builds")


def get_raw_version(ces_version: str) -> str:
    m = re.match(r"^ces-v(.*)$", ces_version)
    if m is None:
        raise MalformedVersionError()
    return m.group(1)


class DescriptorEntry:
    build_name: str
    build_type: str
    version_desc: VersionDescriptor
    image_desc: ImageDescriptor

    def __init__(
        self,
        build_name: str,
        build_type: str,
        version_desc: VersionDescriptor,
        image_desc: ImageDescriptor,
    ):
        self.build_name = build_name
        self.build_type = build_type
        self.version_desc = version_desc
        self.image_desc = image_desc
        self.version_desc = version_desc
        self.image_desc = image_desc


async def get_descriptor_entry(
    build_name: str, build_type: str, path: Path
) -> DescriptorEntry:
    version_desc = VersionDescriptor.read(path)
    raw_version = get_raw_version(version_desc.version)
    image_desc = await get_image_desc(raw_version)
    return DescriptorEntry(build_name, build_type, version_desc, image_desc)


def attempt_sync_images(
    desc: ImageDescriptor,
    secrets_path: Path,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
) -> bool:
    try:
        secrets = SecretsVaultMgr(
            secrets_path,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit=vault_transit,
        )

    except VaultError as e:
        log.error(f"error initializing vault: {e}")
        sys.exit(1)
    except Exception as e:
        log.error(f"unknown error: {e}")
        sys.exit(1)

    for image in desc.images:
        log.info(f"handling '{image.src}' to '{image.dst}")
        try:
            sync_image(image.src, image.dst, secrets, force=False, dry_run=False)
        except CESError as e:
            log.error(f"error copying images: {e}")
            return False
        except Exception as e:
            log.error(f"unknown error: {e}")
            return False

        log.info(f"handled image from '{image.src}' to '{image.dst}'")

    return True


async def _handle_builds(
    base_path: str,
    base_sha: str,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
) -> None:
    secrets_path = Path(base_path).joinpath("secrets.json")
    if not secrets_path.exists():
        log.error(f"missing secrets file at '{secrets_path}'")
        sys.exit(errno.ENOENT)

    try:
        modified, deleted = await get_git_modified_paths(
            base_sha, "HEAD", in_repo_path=base_path
        )
    except GitError as e:
        log.error(f"unable to obtain modified paths: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    for build_desc_file in deleted:
        if build_desc_file.suffix != ".json":
            continue
        log.info(f"descriptor for '{build_desc_file.name}' deleted")

    for build_desc_file in modified:
        if build_desc_file.suffix != ".json":
            continue

        if not build_desc_file.exists():
            log.error(f"build descriptor file does not exist at '{build_desc_file}'")
            continue

        log.info(f"info: process descriptor for '{build_desc_file.name}'")
        build_name = build_desc_file.stem
        build_type = build_desc_file.parent.name

        try:
            desc = await get_descriptor_entry(build_name, build_type, build_desc_file)
        except MalformedVersionError:
            log.error(f"malformed CES version '{build_name}'")
            sys.exit(1)
        except NoSuchVersionError:
            log.error(f"images not found for CES version '{build_name}'")
            continue

        print("=> handle build descriptor:")
        print(f"-       name: {desc.build_name}")
        print(f"-       type: {desc.build_type}")
        print(f"-    version: {desc.version_desc.version}")
        print("- components:")
        for c in desc.version_desc.components:
            print(f"-- {c.name}: {c.ref}")
        print("- images:")
        for img in desc.image_desc.images:
            print(f"-- needs: {img.dst}")

        log.info("attempt to sync required images")
        res = attempt_sync_images(
            desc.image_desc,
            secrets_path,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit,
        )
        if not res:
            log.error(f"failed synchronizing required images for '{desc.build_name}'")
            continue
        log.info(f"images for '{desc.build_name}' synchronized")

        # TODO: call 'ces-build.sh' with info for this build
    pass


@click.command()
@click.option("-d", "--debug", is_flag=True)
@click.option("--base-path", envvar="VERSIONS_BASE_PATH", type=str, required=True)
@click.option("--base-sha", envvar="BUILD_BASE_SHA", type=str, required=True)
@click.option("--vault-addr", envvar="VAULT_ADDR", type=str, required=True)
@click.option("--vault-role-id", envvar="VAULT_ROLE_ID", type=str, required=True)
@click.option("--vault-secret-id", envvar="VAULT_SECRET_ID", type=str, required=True)
@click.option("--vault-transit", envvar="VAULT_TRANSIT", type=str, required=True)
def main(
    debug: bool,
    base_path: str,
    base_sha: str,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
):
    if debug:
        root_logger.setLevel(logging.DEBUG)

    log.debug(f"base_path: {base_path}, base_sha: {base_sha}")
    asyncio.run(
        _handle_builds(
            base_path,
            base_sha,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit,
        )
    )


if __name__ == "__main__":
    main()
