# cbsbuild - commands - versions
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
from pathlib import Path

import click

from cbscore.cmds import logger as parent_logger
from cbscore.cmds import with_config
from cbscore.config import Config, ConfigError
from cbscore.errors import CESError, NoSuchVersionError
from cbscore.images.desc import get_image_desc
from cbscore.releases import ReleaseError
from cbscore.releases.s3 import list_releases
from cbscore.utils.git import GitError, get_git_repo_root, get_git_user
from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets.mgr import SecretsMgr
from cbscore.versions.create import version_create_helper
from cbscore.versions.errors import VersionError
from cbscore.versions.utils import VersionType

logger = parent_logger.getChild("versions")


async def version_create(
    version: str,
    version_type_name: str,
    component_refs: list[str],
    components_paths: list[Path],
    component_uri_overrides: list[str],
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
        desc = version_create_helper(
            version,
            version_type_name,
            component_refs,
            components_paths,
            component_uri_overrides,
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
        sys.exit(errno.ENOTRECOVERABLE)

    version_type_dir_name = version_type_name

    click.echo(f"version: {desc.version}")
    click.echo(f"version title: {desc.title}")

    repo_path = await get_git_repo_root()
    version_path = (
        repo_path.joinpath("_versions")  # FIXME: make this configurable
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


async def versions_list(
    secrets: SecretsMgr,
    s3_address_url: str,
    verbose: bool,
) -> None:
    """List releases from S3."""
    logger.info("listing s3 objects")

    try:
        releases = await list_releases(secrets, s3_address_url)
    except ReleaseError as e:
        click.echo(f"error obtaining releases: {e}")
        sys.exit(1)

    for version, entry in releases.items():
        click.echo(f"> release: {version}")

        if not verbose:
            continue

        for arch, build in entry.builds.items():
            click.echo(f"  - architecture: {arch.value}")
            click.echo(f"    build type: {build.build_type.value}")
            click.echo(f"    OS version: {build.os_version}")
            click.echo("    components:")

            for comp_name, comp in build.components.items():
                click.echo(f"      - {comp_name}:")
                click.echo(f"        version:  {comp.version}")
                click.echo(f"        sha1: {comp.sha1}")
                click.echo(f"        repo url: {comp.repo_url}")
                click.echo(f"        loc: {comp.artifacts.loc}")
            pass


@click.group("versions", help="Manipulate version descriptors.")
def cmd_versions_grp() -> None:
    pass


_version_types_str = ", ".join([t.value for t in VersionType])
_version_create_help_msg = f"""Creates a new version descriptor file.

Requires a VERSION to be provided, which this descriptor describes.
VERSIONs must follow the format

    [prefix-]vM.m.p[-suffix]

where 'M' is the major version, 'm' is the minor version, and 'p' the
patch version.

Requires all components to be passed as '--component NAME@VERSION', individually.

The version can be of any of the following build types:

   {_version_types_str}
"""


@cmd_versions_grp.command("create", help=_version_create_help_msg)
@click.argument("version", type=str)
@click.option(
    "-t",
    "--type",
    "version_type",
    type=str,
    help="Type of version to be built",
    required=False,
    metavar="TYPE",
    default="dev",
    show_default=True,
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
    "--components-path",
    "components_paths",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        resolve_path=True,
        path_type=Path,
    ),
    multiple=True,
    required=False,
    metavar="PATH",
    help="Path to directory holding component definitions",
)
@click.option(
    "-o",
    "--override-component-uri",
    "component_uri_overrides",
    type=str,
    multiple=True,
    help="Override component's URIs",
    required=False,
    metavar="COMPONENT=URI",
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
def cmd_versions_create(
    version: str,
    version_type: str,
    components: tuple[str, ...],
    components_paths: tuple[Path, ...],
    component_uri_overrides: tuple[str, ...],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
):
    asyncio.run(
        version_create(
            version,
            version_type,
            list(components),
            list(components_paths),
            list(component_uri_overrides),
            distro,
            el_version,
            registry,
            image_name,
            image_tag,
        )
    )


@cmd_versions_grp.command("list", help="List known release versions from S3")
@click.option(
    "-v",
    "--verbose",
    is_flag=True,
    default=False,
    required=False,
    help="Show additional release information",
)
@click.option(
    "--from",
    "s3_address_url",
    help="Address to the S3 service to list from.",
    type=str,
    required=True,
    metavar="ADDRESS",
)
@with_config
def cmd_versions_list(
    config: Config,
    verbose: bool,
    s3_address_url: str,
) -> None:
    try:
        vault_config = config.get_vault_config()
    except Exception as e:
        logger.error(f"unable to obtain vault config: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        secrets = SecretsMgr(config.get_secrets(), vault_config=vault_config)
    except ConfigError as e:
        click.echo(f"error obtaining secrets: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except SecretsMgrError as e:
        logger.error(f"error logging in to vault: {e}")
        sys.exit(errno.EACCES)

    asyncio.run(versions_list(secrets, s3_address_url, verbose))

    pass
