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
from ceslib.logger import log as root_logger
from ceslib.runner import RunnerError, runner
from ceslib.versions.desc import VersionDescriptor

log = root_logger.getChild("ces-build")


@click.group()
@click.option(
    "-d", "--debug", help="Enable debug output", is_flag=True, envvar="CBS_DEBUG"
)
def main(debug: bool) -> None:
    if debug:
        root_logger.setLevel(logging.DEBUG)


@main.command("build", help="Start a containerized build")
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
    "--timeout",
    help="Specify how long the build should be allowed to take",
    type=float,
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
    timeout: float | None,
    skip_build: bool,
    force: bool,
) -> None:
    our_dir = Path(sys.argv[0]).parent.parent
    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(
            runner(
                desc_path,
                our_dir,
                secrets_path,
                scratch_dir,
                scratch_containers_dir,
                components_dir,
                containers_dir,
                vault_addr,
                vault_role_id,
                vault_secret_id,
                vault_transit,
                ccache_path=ccache_dir,
                timeout=timeout,
                upload=upload,
                skip_build=skip_build,
                force=force,
            )
        )
    except (RunnerError, Exception):
        log.exception(f"error building '{desc_path}'")
        sys.exit(1)


@main.group("runner", help="Build Runner related operations")
def runner_grp() -> None:
    pass


@runner_grp.command(
    "build",
    help="""Perform a build (internal use).

Should not be called by the user directly. Use 'build' instead.
""",
)
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
def runner_build(
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
    except CESError:
        log.exception("unable to read descriptor")
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
    except Exception:
        log.exception("unable to run build")
        sys.exit(1)


if __name__ == "__main__":
    main()
