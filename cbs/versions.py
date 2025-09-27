#!/usr/bin/env python3

# Handles CES declarative versions
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
import sys
from pathlib import Path

import click
from ceslib.errors import CESError, NoSuchVersionError
from ceslib.images.desc import get_image_desc
from ceslib.logger import logger as root_logger
from ceslib.releases import ReleaseError
from ceslib.releases.s3 import list_releases
from ceslib.utils.git import GitError, get_git_repo_root, get_git_user
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError
from ceslib.versions.create import VersionType, component_repos, create
from ceslib.versions.errors import VersionError

logger = root_logger.getChild("builds")


@click.group()
@click.option("-d", "--debug", envvar="CES_TOOL_DEBUG", is_flag=True)
def main(debug: bool) -> None:
    if debug:
        root_logger.setLevel(logging.DEBUG)
    pass


async def _create(
    version: str,
    version_types: list[str],
    components: list[str],
    component_overrides: list[str],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
) -> None:
    try:
        user_name, user_email = await get_git_user()
    except GitError as e:
        click.echo(f"error obtaining git user info: {e}")
        sys.exit(1)

    try:
        version_type, desc = create(
            version,
            version_types,
            components,
            component_overrides,
            distro,
            el_version,
            registry,
            image_name,
            image_tag,
            user_name,
            user_email,
        )
    except VersionError as e:
        click.echo(f"error creating version descriptor: {e}", err=True)
        sys.exit(1)

    version_type_dir_name = (
        "testing" if version_type == VersionType.TESTING else "release"
    )

    click.echo(f"version: {desc.version}")
    click.echo(f"version title: {desc.title}")

    repo_path = await get_git_repo_root()
    version_path = (
        repo_path.joinpath("versions")
        .joinpath(version_type_dir_name)
        .joinpath(f"{desc.version}.json")
    )
    if version_path.exists():
        click.echo(f"version for {desc.version} already exists", err=True)
        sys.exit(errno.EEXIST)

    version_path.parent.mkdir(parents=True, exist_ok=True)

    desc_json = desc.model_dump_json(indent=2)
    click.echo(desc_json)

    try:
        desc.write(version_path)
    except Exception as e:
        click.echo(f"unable to write descriptor at '{version_path}': {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"-> written to {version_path}")

    # check if image descriptor for this version exists
    try:
        _ = await get_image_desc(desc.version)
    except NoSuchVersionError:
        click.echo(f"image descriptor for version '{desc.version}' missing")
    except CESError as e:
        click.echo(
            f"error obtaining image descriptor for '{desc.version}': {e}", err=True
        )


async def _list(
    secrets: SecretsVaultMgr,
    verbose: bool,
) -> None:
    """List releases from S3."""
    logger.info("listing s3 objects")

    try:
        releases = await list_releases(secrets)
    except ReleaseError as e:
        click.echo(f"error obtaining releases: {e}")
        sys.exit(1)

    for version, entry in releases.items():
        click.echo(f"> release: {version}")
        click.echo(f"  el version: {entry.el_version}")

        if not verbose:
            continue

        for comp_name, component in entry.components.items():
            click.echo(f"  - {comp_name}:")
            click.echo(f"    version:  {component.version}")
            click.echo(f"    location: {component.loc}")
            click.echo(f"    repo:     {component.repo_url}")


_create_help_msg = f"""Creates a new version descriptor file.

Requires a VERSION to be provided, which this descriptor describes.
VERSION must include the "CES" prefix if a CES version is intended. Otherwise,
it can be free-form as long as it starts with a 'v' (such as, 'v18.2.4').

Requires at least one '--type TYPE=N' pair, specifying which type of release
the version refers to.

Requires all components to be passed as '--component NAME@VERSION', individually.

Available components: {", ".join(component_repos.keys())}.
"""


@main.command("create", help=_create_help_msg)
@click.argument("version", type=str)
@click.option(
    "-t",
    "--type",
    "build_types",
    type=str,
    multiple=True,
    help="Type of build, and its iteration",
    required=False,
    metavar="TYPE=N",
)
@click.option(
    "-c",
    "--component",
    "components",
    type=str,
    multiple=True,
    required=True,
    metavar="NAME@VERSION",
    help="Component's versions (e.g., 'ceph@ces-v24.11.0-ga.1')",
)
@click.option(
    "-o",
    "--override-component",
    "component_overrides",
    type=str,
    multiple=True,
    help="Override component's locations",
    required=False,
    metavar="COMPONENT=URL",
)
@click.option(
    "--distro",
    type=str,
    help="Distribution to use for this release",
    required=False,
    default="rockylinux:9",
    metavar="NAME",
)
@click.option(
    "--el-version",
    type=int,
    help="Distribution EL version",
    required=False,
    default=9,
    metavar="VERSION",
)
@click.option(
    "--registry",
    type=str,
    help="Registry for this release's image",
    required=False,
    default="harbor.clyso.com",
    metavar="URL",
)
@click.option(
    "--image-name",
    type=str,
    help="Name for this release's image",
    required=False,
    default="ces/ceph/ceph",
    metavar="NAME",
)
@click.option(
    "--image-tag",
    type=str,
    help="Tag for this release's image",
    required=False,
    metavar="TAG",
)
def build_create(
    version: str,
    build_types: list[str],
    components: list[str],
    component_overrides: list[str],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
):
    asyncio.run(
        _create(
            version,
            build_types,
            components,
            component_overrides,
            distro,
            el_version,
            registry,
            image_name,
            image_tag,
        )
    )

    pass


@main.command("list", help="List known release versions from S3")
@click.option(
    "-v",
    "--verbose",
    is_flag=True,
    default=False,
    required=False,
    help="Show additional release information",
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
    "--vault-addr",
    envvar="VAULT_ADDR",
    type=str,
    required=True,
    metavar="ADDRESS/URL",
    help="Vault address",
)
@click.option(
    "--vault-role-id",
    envvar="VAULT_ROLE_ID",
    type=str,
    required=True,
    metavar="ROLE_ID",
    help="Vault Role ID",
)
@click.option(
    "--vault-secret-id",
    envvar="VAULT_SECRET_ID",
    type=str,
    required=True,
    metavar="SECRET_ID",
    help="Vault Secret ID",
)
def list_versions(
    verbose: bool,
    secrets_path: Path,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
) -> None:
    try:
        secrets = SecretsVaultMgr(
            secrets_path,
            vault_addr,
            vault_role_id,
            vault_secret_id,
            vault_transit=None,
        )
    except VaultError:
        logger.exception("error logging in to vault")
        sys.exit(1)

    asyncio.run(_list(secrets, verbose))

    pass


if __name__ == "__main__":
    main()
