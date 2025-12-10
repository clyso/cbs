# cbc - commands - builds
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
import logging
import sys
from typing import cast

import click
import pydantic

from cbc import CBCError
from cbc.auth import auth_whoami
from cbc.client import CBCClient, CBCConnectionError, CBCPermissionDeniedError
from cbc.cmds import endpoint, pass_config, pass_logger, update_ctx
from cbscore.versions.errors import VersionError
from cbscore.versions.utils import VersionType, get_version_type, parse_component_refs
from cbsdcore.api.responses import NewBuildResponse
from cbsdcore.auth.user import UserConfig
from cbsdcore.builds.types import BuildEntry
from cbsdcore.versions import (
    BuildArch,
    BuildArtifactType,
    BuildComponent,
    BuildDescriptor,
    BuildDestImage,
    BuildSignedOffBy,
    BuildTarget,
)

# pyright: reportUnusedParameter=false, reportUnusedFunction=false


@endpoint("/components/")
def _list_components(logger: logging.Logger, client: CBCClient, ep: str) -> list[str]:
    try:
        r = client.get(ep)
        lst = cast(list[str], r.json())
        logger.debug(f"obtained components: {lst}")
    except CBCError as e:
        logger.error(f"unable to obtain component list: {e}")
        raise e from None

    return lst


@endpoint("/builds/new")
def _build_new(
    logger: logging.Logger, client: CBCClient, ep: str, desc: BuildDescriptor
) -> NewBuildResponse:
    data = desc.model_dump(mode="json")
    try:
        r = client.post(ep, data)
        res = r.json()  # pyright: ignore[reportAny]
        logger.debug(f"new build: {res}")
    except CBCError as e:
        logger.error(f"unable to create new build: {e}")
        raise e from None

    try:
        return NewBuildResponse.model_validate(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.error(msg)
        raise CBCError(msg) from None


@endpoint("/builds/status")
def _build_list(
    logger: logging.Logger, client: CBCClient, ep: str, all: bool
) -> list[BuildEntry]:
    try:
        r = client.get(ep, params={"all": all})
        res = r.json()  # pyright: ignore[reportAny]
    except CBCError as e:
        logger.error(f"unable to list builds: {e}")
        raise e from None

    ta = pydantic.TypeAdapter(list[BuildEntry])
    try:
        return ta.validate_python(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.error(msg)
        raise CBCError(msg) from None


@endpoint("/builds/revoke")
def _build_revoke(
    logger: logging.Logger, client: CBCClient, ep: str, build_id: str, force: bool
) -> None:
    try:
        params = {"force": force} if force else None
        _ = client.delete(f"{ep}/{build_id}", params=params)
    except CBCError as e:
        logger.error(f"unable to revoke build '{build_id}': {e}")
        raise e from None


@click.group("build", help="build related commands")
@update_ctx
def cmd_build() -> None:
    pass


@cmd_build.command("components", help="List available components")
@update_ctx
@pass_logger
@pass_config
def cmd_build_components_list(config: UserConfig, logger: logging.Logger) -> None:
    try:
        res = _list_components(logger, config)
    except CBCError as e:
        click.echo(f"error listing components: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"available components: {res}")


@cmd_build.command("new", help="Create new build")
@click.argument("version", type=str, metavar="VERSION", required=True)
@click.option(
    "-t",
    "--type",
    "version_type_name",
    type=click.Choice([t.value for t in VersionType], case_sensitive=True),
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
    help="Component's version (e.g., 'ceph@abcde1234')",
)
@click.option(
    "--override-component",
    "component_overrides",
    type=str,
    multiple=True,
    required=False,
    metavar="COMPONENT=URL",
    help="Override component's location",
)
@click.option(
    "--distro",
    type=str,
    required=False,
    default="rockylinux:9",
    metavar="NAME",
    help="Distribution to use for this release",
)
@click.option(
    "--el-version",
    type=int,
    required=False,
    default=9,
    metavar="VERSION",
    help="Distribution's EL version",
)
@click.option(
    "--registry",
    type=str,
    required=False,
    default="harbor.clyso.com",
    metavar="URL",
    help="Registry for this release's image",
)
@click.option(
    "--image-name",
    type=str,
    required=False,
    default="ces/ceph/ceph",
    metavar="NAME",
    help="Name for this release's image",
)
@click.option(
    "--image-tag",
    type=str,
    required=False,
    metavar="TAG",
    help="Tag for this release's image",
)
@update_ctx
@pass_logger
@pass_config
def cmd_build_new(
    config: UserConfig,
    logger: logging.Logger,
    version: str,
    version_type_name: str,
    components: tuple[str, ...],
    component_overrides: tuple[str, ...],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
) -> None:
    try:
        email, name = auth_whoami(logger, config)
    except CBCConnectionError as e:
        click.echo(f"connection error: {e}", err=True)
        sys.exit(errno.ECONNREFUSED)
    except CBCPermissionDeniedError as e:
        click.echo(f"permission denied: {e}", err=True)
        sys.exit(errno.EACCES)
    except Exception as e:
        click.echo(f"error obtaining user's info: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        version_type = get_version_type(version_type_name)
    except VersionError as e:
        click.echo(f"error parsing version type: {e}", err=True)
        sys.exit(errno.EINVAL)

    try:
        component_refs = parse_component_refs(list(components))
    except VersionError as e:
        click.echo(f"error parsing components: {e}", err=True)
        sys.exit(errno.EINVAL)

    uri_overrides: dict[str, str] = {}
    for uri_override in component_overrides:
        entries = uri_override.split("=", maxsplit=1)
        if len(entries) != 2:
            click.echo(f"malformed component URI override: '{uri_override}'", err=True)
            sys.exit(errno.EINVAL)

        comp, uri = entries
        if comp not in component_refs:
            click.echo(f"ignoring URI for missing component '{comp}'", err=True)
            continue

        uri_overrides[comp] = uri

    components_lst: list[BuildComponent] = []
    for comp_name, comp_ref in component_refs.items():
        components_lst.append(
            BuildComponent(
                name=comp_name,
                ref=comp_ref,
                repo=uri_overrides.get(comp_name),
            )
        )

    image_tag = image_tag or version

    desc = BuildDescriptor(
        version=version,
        signed_off_by=BuildSignedOffBy(user=name, email=email),
        version_type=version_type,
        dst_image=BuildDestImage(name=image_name, tag=image_tag),
        components=components_lst,
        build=BuildTarget(
            distro=distro,
            os_version=f"el{el_version}",
            artifact_type=BuildArtifactType.rpm,
            arch=BuildArch.x86_64,
        ),
    )

    try:
        res = _build_new(logger, config, desc)
    except CBCError as e:
        click.echo(f"error triggering build: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"""
triggered build:
    type: {version_type_name}
 task id: {res.task_id}
   state: {res.state}
""")


@cmd_build.command("list", help="List builds from the build service")
@click.option("--all", is_flag=True, default=False, help="List all known builds")
@update_ctx
@pass_logger
@pass_config
def cmd_build_list(config: UserConfig, logger: logging.Logger, all: bool) -> None:
    try:
        lst = _build_list(logger, config, all)
    except CBCError as e:
        click.echo(f"error obtaining build list: {e}", err=True)
        sys.exit(1)

    if not lst:
        click.echo("no builds found")
        return

    for entry in lst:
        click.echo("---")
        click.echo(f" build id: {entry.task_id}")
        click.echo(f"     user: {entry.user}")
        click.echo(f"    state: {entry.state}")
        click.echo(f"submitted: {entry.submitted}")
        click.echo(f" finished: {entry.finished}")

    pass


@cmd_build.command("revoke", help="Revoke an on-going build")
@click.argument("build_id", type=str, required=True, metavar="ID")
@click.option(
    "--force",
    is_flag=True,
    required=False,
    default=False,
    help="Force revoking build, regardless of whom has created it",
)
@update_ctx
@pass_logger
@pass_config
def cmd_build_revoke(
    config: UserConfig, logger: logging.Logger, build_id: str, force: bool
) -> None:
    try:
        _build_revoke(logger, config, build_id, force)
    except CBCError as e:
        click.echo(f"error revoking build '{build_id}': {e}", err=True)
        sys.exit(1)

    click.echo(f"successfully revoked build '{build_id}'")
