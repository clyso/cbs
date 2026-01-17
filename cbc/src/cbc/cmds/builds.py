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

import click
import pydantic
from cbsdcore.api.responses import AvailableComponent, NewBuildResponse
from cbsdcore.auth.user import UserConfig
from cbsdcore.builds.types import BuildEntry, BuildID
from cbsdcore.versions import (
    BuildDescriptor,
)

from cbc import CBCError
from cbc.client import CBCClient
from cbc.cmds import endpoint, logs, pass_config, pass_logger, periodic, update_ctx
from cbc.cmds._shared import build_descriptor_options, new_build_descriptor_helper

# pyright: reportUnusedParameter=false, reportUnusedFunction=false


@endpoint("/components/")
def _list_components(
    logger: logging.Logger, client: CBCClient, ep: str
) -> dict[str, AvailableComponent]:
    try:
        r = client.get(ep)
        res = r.json()  # pyright: ignore[reportAny]
        logger.debug(f"obtained components: {res}")
    except CBCError as e:
        logger.error(f"unable to obtain component list: {e}")
        raise e from None

    ta = pydantic.TypeAdapter(dict[str, AvailableComponent])
    try:
        return ta.validate_python(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.error(msg)
        raise CBCError(msg) from None


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
) -> list[tuple[int, BuildEntry]]:
    try:
        r = client.get(ep, params={"all": all})
        res = r.json()  # pyright: ignore[reportAny]
    except CBCError as e:
        logger.error(f"unable to list builds: {e}")
        raise e from None

    ta = pydantic.TypeAdapter(list[tuple[int, BuildEntry]])
    try:
        return ta.validate_python(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.error(msg)
        raise CBCError(msg) from None


@endpoint("/builds/revoke")
def _build_revoke(
    logger: logging.Logger, client: CBCClient, ep: str, build_id: BuildID, force: bool
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

    for comp_name, comp in res.items():
        click.echo(f"\n-> {comp_name}")
        click.echo(f"   repo: {comp.default_repo}")
        click.echo(f"   versions: {comp.versions}")


@cmd_build.command("new", help="Create new build")
@click.argument("version", type=str, metavar="VERSION", required=True)
@build_descriptor_options
@update_ctx
@pass_logger
@pass_config
def cmd_build_new(
    config: UserConfig,
    logger: logging.Logger,
    version: str,
    version_type_name: str,
    version_channel: str,
    components: tuple[str, ...],
    component_overrides: tuple[str, ...],
    distro: str,
    el_version: int,
    # registry: str,  # currently unused?
    image_name: str,
    image_tag: str | None,
) -> None:
    desc = new_build_descriptor_helper(
        config,
        logger,
        version=version,
        version_type_name=version_type_name,
        version_channel=version_channel,
        components=components,
        component_overrides=component_overrides,
        distro=distro,
        el_version=el_version,
        image_name=image_name,
        image_tag=image_tag,
    )

    click.echo(f"""
requesting build for:

   version: {desc.version}
   channel: {desc.channel}
      type: {desc.version_type}
     image: {desc.dst_image.name}:{desc.dst_image.tag}
components: {", ".join([comp.name for comp in desc.components])}
    distro: {desc.build.distro}
os version: {desc.build.os_version}

""")

    try:
        res = _build_new(logger, config, desc)
    except CBCError as e:
        click.echo(f"error triggering build: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"""
triggered build:
    type: {desc.version_type}
build id: {res.build_id}
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
        sys.exit(errno.ENOTRECOVERABLE)

    if not lst:
        click.echo("no builds found")
        return

    for build_id, entry in lst:
        click.echo("---")
        click.echo(f" build id: {build_id}")
        click.echo(f"     user: {entry.user}")
        click.echo(f"    state: {entry.state}")
        click.echo(f"submitted: {entry.submitted}")
        click.echo(f" finished: {entry.finished}")

    pass


@cmd_build.command("revoke", help="Revoke an on-going build")
@click.argument("build_id", type=BuildID, required=True, metavar="ID")
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
    config: UserConfig, logger: logging.Logger, build_id: BuildID, force: bool
) -> None:
    try:
        _build_revoke(logger, config, build_id, force)
    except CBCError as e:
        click.echo(f"error revoking build '{build_id}': {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"successfully revoked build '{build_id}'")


cmd_build.add_command(periodic.cmd_periodic_build_grp)
cmd_build.add_command(logs.cmd_build_logs_grp)
