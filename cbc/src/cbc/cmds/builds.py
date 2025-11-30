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
from cbc.client import CBCClient
from cbc.cmds import endpoint, pass_config, pass_logger, update_ctx
from cbscore.versions.desc import VersionDescriptor
from cbsdcore.api.responses import NewBuildResponse
from cbsdcore.auth.user import UserConfig
from cbsdcore.builds.types import BuildEntry

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
    logger: logging.Logger, client: CBCClient, ep: str, desc: VersionDescriptor
) -> NewBuildResponse:
    data = desc.model_dump(mode="json")
    try:
        r = client.post(ep, data)
        res = r.json()  # pyright: ignore[reportAny]
        logger.debug(f"new build: {res}")
    except CBCError as e:
        logger.exception("unable to create new build")
        raise e  # noqa: TRY201

    try:
        return NewBuildResponse.model_validate(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.exception(msg)
        raise CBCError(msg) from None


@endpoint("/builds/status")
def _build_list(
    logger: logging.Logger, client: CBCClient, ep: str, all: bool
) -> list[BuildEntry]:
    try:
        r = client.get(ep, params={"all": all})
        res = r.json()  # pyright: ignore[reportAny]
    except CBCError as e:
        logger.exception("unable to list builds")
        raise e  # noqa: TRY201

    ta = pydantic.TypeAdapter(list[BuildEntry])
    try:
        return ta.validate_python(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.exception(msg)
        raise CBCError(msg) from None


@endpoint("/builds/abort")
def _build_abort(
    logger: logging.Logger, client: CBCClient, ep: str, build_id: str, force: bool
) -> None:
    try:
        params = {"force": force} if force else None
        _ = client.delete(f"{ep}/{build_id}", params=params)
    except CBCError as e:
        logger.exception(f"unable to abort build '{build_id}'")
        raise e  # noqa: TRY201


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
    version_type: str,
    components: tuple[str, ...],
    component_overrides: tuple[str, ...],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
) -> None:
    pass
    # try:
    #     email, name = auth_whoami(logger, config)
    # except Exception as e:
    #     click.echo(f"error obtaining user's info: {e}", err=True)
    #     sys.exit(1)
    #
    # try:
    #     version_type, desc = create(
    #         version,
    #         version_type,
    #         components,
    #         component_overrides,
    #         distro,
    #         el_version,
    #         registry,
    #         image_name,
    #         image_tag,
    #         name,
    #         email,
    #     )
    # except (VersionError, Exception) as e:
    #     click.echo(f"error creating version descriptor: {e}")
    #     sys.exit(1)
    #
    # try:
    #     res = _build_new(logger, config, desc)
    # except CBCError as e:
    #     click.echo(f"error triggering build: {e}")
    #     sys.exit(1)
    #
    # click.echo(f"version type: {version_type.name}")
    # click.echo(f"     task id: {res.task_id}")
    # click.echo(f"       state: {res.state}")


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


@cmd_build.command("abort", help="Abort an existing build")
@click.argument("build_id", type=str, required=True, metavar="ID")
@click.option(
    "--force",
    is_flag=True,
    required=False,
    default=False,
    help="Force aborting build, regardless of whom has created it",
)
@update_ctx
@pass_logger
@pass_config
def cmd_build_abort(
    config: UserConfig, logger: logging.Logger, build_id: str, force: bool
) -> None:
    try:
        _build_abort(logger, config, build_id, force)
    except CBCError as e:
        click.echo(f"error aborting build '{build_id}': {e}", err=True)
        sys.exit(1)

    click.echo(f"successfully aborted build '{build_id}'")
