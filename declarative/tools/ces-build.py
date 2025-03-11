#!/usr/bin/env python3

# Builds a declarative version, using a container
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
import sys
from pathlib import Path

import click
from ceslib.builder.builder import Builder
from ceslib.errors import CESError
from ceslib.logging import log as root_logger
from ceslib.utils.podman import PodmanError, podman_run
from ceslib.versions.desc import VersionDescriptor

log = root_logger.getChild("ces-build")


@click.group()
@click.option("-d", "--debug", help="Enable debug output", is_flag=True)
def main(debug: bool) -> None:
    if debug:
        root_logger.setLevel(logging.DEBUG)


@main.command()
@click.argument(
    "desc_path",
    metavar="DESCRIPTOR",
    # help="Descriptor file to build",
    type=click.Path(
        exists=True, dir_okay=False, file_okay=True, readable=True, path_type=Path
    ),
    required=True,
)
@click.option(
    "--secrets",
    "secrets_path",
    help="Path to 'secrets.json'",
    envvar="SECRETS_PATH",
    type=click.Path(
        exists=True,
        dir_okay=False,
        file_okay=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--upload/--no-upload",
    help="Upload artifacts to Clyso's S3",
    is_flag=True,
    default=True,
)
@click.option(
    "--vault-addr",
    envvar="VAULT_ADDR",
    type=str,
    required=True,
)
@click.option(
    "--vault-role-id",
    envvar="VAULT_ROLE_ID",
    type=str,
    required=True,
)
@click.option(
    "--vault-secret-id",
    envvar="VAULT_SECRET_ID",
    type=str,
    required=True,
)
@click.option(
    "--vault-transit",
    envvar="VAULT_TRANSIT",
    type=str,
    required=True,
)
@click.option(
    "--scratch-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--scratch-containers-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--components-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--containers-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--ccache-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=False,
)
@click.option(
    "--skip-build",
    help="Skip building RPMs for components",
    is_flag=True,
    default=False,
)
@click.option(
    "--force",
    help="Force the entire build",
    is_flag=True,
    default=False,
)
def build(
    desc_path: Path,
    secrets_path: Path,
    upload: bool,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
    scratch_dir: Path,
    scratch_containers_dir: Path,
    components_dir: Path,
    containers_dir: Path,
    ccache_dir: Path | None,
    skip_build: bool,
    force: bool,
) -> None:
    log.info(f"build desc: {desc_path}, upload: {upload}")
    log.info(f"with ccache path: {ccache_dir}")

    if not desc_path.exists():
        log.error(f"build descriptor does not exist at '{desc_path}'")

    try:
        desc = VersionDescriptor.read(desc_path)
    except CESError as e:
        log.error(f"unable to read descriptor: {e}")
        sys.exit(1)

    our_dir = Path(sys.argv[0]).parent

    podman_volumes = {
        desc_path.resolve().as_posix(): f"/builder/{desc_path.name}",
        our_dir.resolve().as_posix(): "/builder/tools",
        scratch_dir.resolve().as_posix(): "/builder/scratch",
        scratch_containers_dir.resolve().as_posix(): "/var/lib/containers:Z",
        secrets_path.resolve().as_posix(): "/builder/secrets.json",
        components_dir.resolve().as_posix(): "/builder/components",
        containers_dir.resolve().as_posix(): "/builder/containers",
    }

    podman_args: list[str] = ["--desc", f"/builder/{desc_path.name}"]

    if ccache_dir:
        ccache_dir_str = ccache_dir.resolve().as_posix()
        podman_volumes[ccache_dir_str] = "/builder/ccache"
        podman_args.extend(["--ccache-path", "/builder/ccache"])

    if skip_build:
        podman_args.append("--skip-build")

    if upload:
        podman_args.append("--upload")

    if force:
        podman_args.append("--force")

    try:
        loop = asyncio.new_event_loop()
        retcode, stdout, stderr = loop.run_until_complete(
            podman_run(
                image=desc.distro,
                env={
                    "VAULT_ADDR": vault_addr,
                    "VAULT_ROLE_ID": vault_role_id,
                    "VAULT_SECRET_ID": vault_secret_id,
                    "VAULT_TRANSIT": vault_transit,
                    "WITH_DEBUG": "1"
                    if log.getEffectiveLevel() == logging.DEBUG
                    else "0",
                },
                args=podman_args,
                volumes=podman_volumes,
                devices={"/dev/fuse": "/dev/fuse:rw"},
                entrypoint="/builder/tools/ctr-build-entrypoint.sh",
                use_user_ns=False,
                use_host_network=True,
                unconfined=True,
            )
        )
        log.debug(f"podman run: rc = {retcode}")
        log.debug(f"podman run: stdout = {stdout}")
        log.debug(f"podman run: stderr = {stderr}")

    except PodmanError as e:
        log.error(f"error running build image: {e}")
        sys.exit(1)
    except Exception as e:
        log.error(f"unknown error running build image: {e}")
        sys.exit(1)
    pass


@main.command()
@click.option(
    "--desc",
    "desc_path",
    type=click.Path(
        exists=True, dir_okay=False, file_okay=True, readable=True, path_type=Path
    ),
    required=True,
)
@click.option(
    "--vault-addr",
    envvar="VAULT_ADDR",
    type=str,
    required=True,
)
@click.option(
    "--vault-role-id",
    envvar="VAULT_ROLE_ID",
    type=str,
    required=True,
)
@click.option(
    "--vault-secret-id",
    envvar="VAULT_SECRET_ID",
    type=str,
    required=True,
)
@click.option(
    "--vault-transit",
    envvar="VAULT_TRANSIT",
    type=str,
    required=True,
)
@click.option(
    "--scratch-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--components-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--containers-dir",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--secrets-path",
    type=click.Path(
        exists=True,
        dir_okay=False,
        file_okay=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "--ccache-path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=False,
)
@click.option(
    "--upload/--no-upload",
    help="Upload artifacts to Clyso's S3",
    is_flag=True,
    default=True,
)
@click.option(
    "--skip-build",
    help="Skip building RPMs for components",
    is_flag=True,
    default=False,
)
@click.option(
    "--force",
    help="Force the entire build",
    is_flag=True,
    default=False,
)
def ctr_build(
    desc_path: Path,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
    scratch_dir: Path,
    components_dir: Path,
    containers_dir: Path,
    secrets_path: Path,
    ccache_path: Path | None,
    upload: bool,
    skip_build: bool,
    force: bool,
) -> None:
    log.debug(f"desc: {desc_path}")
    log.debug(f"vault addr: {vault_addr}")
    log.debug(f"vault role id: {vault_role_id}")
    log.debug(f"vault transit: {vault_transit}")
    log.debug(f"scratch dir: {scratch_dir}")
    log.debug(f"secrets path: {secrets_path}")
    log.debug(f"upload: {upload}")
    log.debug(f"force: {force}")

    if not desc_path.exists():
        log.error(f"build descriptor does not exist at '{desc_path}'")

    try:
        desc = VersionDescriptor.read(desc_path)
    except CESError as e:
        log.error(f"unable to read descriptor: {e}")
        sys.exit(1)

    builder = Builder(
        desc,
        vault_addr,
        vault_role_id,
        vault_secret_id,
        vault_transit,
        scratch_dir,
        secrets_path,
        components_dir,
        containers_dir,
        upload=upload,
        ccache_path=ccache_path,
        skip_build=skip_build,
        force=force,
    )

    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(builder.run())
    except Exception as e:
        log.error(f"unable to run build: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
