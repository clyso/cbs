#!/usr/bin/env python3

import errno
import logging
import re
import sys
from pathlib import Path

import click
from ceslib.builds.desc import BuildDescriptor
from ceslib.errors import CESError, MalformedVersionError, NoSuchVersionError
from ceslib.images.desc import ImageDescriptor, get_version_desc
from ceslib.images.sync import sync_image
from ceslib.logging import log as root_logger
from ceslib.utils.git import GitError, get_git_modified_paths
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError

log = root_logger.getChild("handle-builds")


def get_raw_version(ces_version: str) -> str:
    m = re.match(r"^ces-v(.*)$", ces_version)
    if m is None:
        raise MalformedVersionError()
    return m.group(1)


class DescriptorEntry:
    build_name: str
    build_type: str
    build_desc: BuildDescriptor
    image_desc: ImageDescriptor

    def __init__(self, build_name: str, build_type: str, path: Path):
        self.build_name = build_name
        self.build_type = build_type
        self.build_desc = BuildDescriptor.read(path)

        # propagate exception
        raw_version = get_raw_version(self.build_desc.version)
        self.image_desc = get_version_desc(raw_version)


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


@click.command()
@click.option("-d", "--debug", is_flag=True)
@click.option("--base-path", envvar="BUILDS_BASE_PATH", type=str, required=True)
@click.option("--base-sha", envvar="BUILDS_BASE_SHA", type=str, required=True)
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

    secrets_path = Path(base_path).joinpath("secrets.json")
    if not secrets_path.exists():
        log.error(f"missing secrets file at '{secrets_path}'")
        sys.exit(errno.ENOENT)

    try:
        modified, deleted = get_git_modified_paths(base_sha, "HEAD", base_path)
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
            desc = DescriptorEntry(build_name, build_type, build_desc_file)
        except MalformedVersionError:
            log.error(f"malformed CES version '{build_name}'")
            sys.exit(1)
        except NoSuchVersionError:
            log.error(f"images not found for CES version '{build_name}'")
            continue

        print("=> handle build descriptor:")
        print(f"-       name: {desc.build_name}")
        print(f"-       type: {desc.build_type}")
        print(f"-    version: {desc.build_desc.version}")
        print("- components:")
        for c in desc.build_desc.components:
            print(f"-- {c.name}: {c.version}")
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


if __name__ == "__main__":
    main()
