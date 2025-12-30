# cbc - commands - periodic builds
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
import uuid

import click
import pydantic
from cbsdcore.api.requests import NewPeriodicBuildTaskRequest
from cbsdcore.auth.user import UserConfig
from cbsdcore.versions import BuildDescriptor

from cbc import CBCError
from cbc.client import CBCClient
from cbc.cmds import endpoint, pass_config, pass_logger, update_ctx
from cbc.cmds._shared import build_descriptor_options, new_build_descriptor_helper


@endpoint("/periodic/build")
def _new_periodic_build(
    logger: logging.Logger,
    client: CBCClient,
    ep: str,
    cron_format: str,
    tag_format: str,
    desc: BuildDescriptor,
    summary: str | None,
) -> uuid.UUID:
    periodic_build_req = NewPeriodicBuildTaskRequest(
        cron_format=cron_format,
        tag_format=tag_format,
        descriptor=desc,
        summary=summary,
    )

    # NOTE: at this point, we have no clear idea why we need to be
    # using 'model_dump(mode="json")' here instead of 'model_dump_json()', but we're
    # following what we are doing for creating a new build (see 'builds.py'). We believe
    # our past-selves had a good reason to do it like this, given how it's not the
    # obvious choice.
    req_data = periodic_build_req.model_dump(mode="json")
    try:
        r = client.post(ep, req_data)
        res = r.json()  # pyright: ignore[reportAny]
        logger.debug(f"obtained periodic build uuid '{res}'")
    except CBCError as e:
        logger.error(f"unable to set up periodic build: {e}")
        raise e from None

    ta = pydantic.TypeAdapter(uuid.UUID)
    try:
        return ta.validate_python(res)
    except pydantic.ValidationError:
        msg = f"error parsing server result: {res}"
        logger.error(msg)
        raise CBCError(msg) from None


@click.group("periodic", help="periodic builds related commands")
@update_ctx
def cmd_periodic_build_grp() -> None:
    pass


@cmd_periodic_build_grp.command("new", help="Create new periodic build")
@click.argument("cron_format", type=str, metavar="CRON_FORMAT", required=True)
@click.argument("tag_format", type=str, metavar="TAG_FORMAT", required=True)
@click.option(
    "-n",
    "--name",
    "version_name",
    type=str,
    help="Build's version name",
    required=True,
    metavar="NAME",
)
@click.option(
    "-m",
    "--summary",
    "summary",
    type=str,
    help="Periodic builds summary description",
    required=False,
    metavar="DESCRIPTION",
)
@build_descriptor_options
@update_ctx
@pass_logger
@pass_config
def cmd_periodic_build_new(
    config: UserConfig,
    logger: logging.Logger,
    cron_format: str,
    tag_format: str,
    version_name: str,
    summary: str | None,
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
        version=version_name,
        version_type_name=version_type_name,
        version_channel=version_channel,
        components=components,
        component_overrides=component_overrides,
        distro=distro,
        el_version=el_version,
        image_name=image_name,
        image_tag=image_tag,
    )

    try:
        res_uuid = _new_periodic_build(
            logger,
            config,
            cron_format,
            tag_format,
            desc,
            summary,
        )
    except CBCError as e:
        click.echo(f"error setting up new periodic build: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"""
created new periodic build '{res_uuid}':

summary: {summary if summary else "N/A"}
 period: {cron_format}

version name: {desc.version}
     channel: {desc.channel}
        type: {desc.version_type}
  components: {", ".join([comp.name for comp in desc.components])}
      distro: {desc.build.distro}
  os version: {desc.build.os_version}

image:
  name: {desc.dst_image.name}
   tag: {tag_format}
""")

    pass
