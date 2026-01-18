# cbc - commands - builds logs
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
import time
from pathlib import Path

import click
import pydantic
from cbsdcore.api.responses import BuildLogsFollowResponse
from cbsdcore.auth.user import UserConfig

from cbc import CBCError
from cbc.client import CBCClient, QueryParams
from cbc.cmds import endpoint, pass_config, pass_logger, update_ctx


def _obtain_log_part(
    logger: logging.Logger,
    client: CBCClient,
    ep: str,
    build_id: int,
    params: QueryParams | None,
) -> BuildLogsFollowResponse:
    """Obtain a part of a log from the server."""
    try:
        r = client.get(ep, params=params)
        res = r.json()  # pyright: ignore[reportAny]
    except CBCError as e:
        logger.error(f"error probing server for build logs: {e}")
        raise e from None

    try:
        return BuildLogsFollowResponse.model_validate(res)
    except pydantic.ValidationError as e:
        msg = f"error parsing server result: {res}\n{e}"
        logger.error(msg)
        raise CBCError(msg) from None


@endpoint("/builds/logs/{build_id}/follow")
def _follow_builds_log(
    logger: logging.Logger,
    client: CBCClient,
    ep: str,
    build_id: int,
    *,
    since: str | None = None,
    max_msgs: int | None = None,
) -> BuildLogsFollowResponse:
    """Probe the server for build log results."""
    real_ep = ep.format(build_id=build_id)
    params: QueryParams = {}
    if since:
        params["since"] = since
    if max_msgs:
        params["n"] = max_msgs

    return _obtain_log_part(
        logger,
        client,
        real_ep,
        build_id,
        params if len(params) > 0 else None,
    )


@endpoint("/builds/logs/{build_id}/tail")
def _tail_builds_log(
    logger: logging.Logger,
    client: CBCClient,
    ep: str,
    build_id: int,
    *,
    max_msgs: int | None = None,
) -> BuildLogsFollowResponse:
    """Obtain build log's tail from the server."""
    real_ep = ep.format(build_id=build_id)
    return _obtain_log_part(
        logger, client, real_ep, build_id, {"n": max_msgs} if max_msgs else None
    )


@endpoint("/builds/logs/{build_id}")
def _download_log_file(
    logger: logging.Logger,
    client: CBCClient,
    ep: str,
    build_id: int,
    *,
    dest_path: Path | None = None,
) -> Path:
    """Download a log file for a given build from the server."""
    real_ep = ep.format(build_id=build_id)

    try:
        with client.download(real_ep) as (fname, response):
            client.maybe_handle_error(response)
            fpath = Path(fname) if fname else None
            dpath = (dest_path or fpath) or Path(f"build-{build_id}.log")

            assert dpath, "no path defined to write file to"
            with dpath.open("w+") as fd:
                for chunk in response.iter_text():
                    _ = fd.write(chunk)

            return dpath
    except CBCError as e:
        raise e from None


@click.group("logs", help="build logs related commands")
def cmd_build_logs_grp() -> None:
    pass


@cmd_build_logs_grp.command("tail", help="Tail a build's log")
@click.argument("build_id", type=int, metavar="ID", required=True)
@click.option(
    "-n",
    "--num-msgs",
    "num_msgs",
    type=int,
    required=False,
    help="Number of context lines",
)
@click.option(
    "-s",
    "--frequency",
    "probe_frequency",
    type=float,
    required=False,
    default=1.0,
    show_default=True,
    help="Frequency to probe server for messages",
)
@click.option(
    "-f",
    "--follow",
    "follow",
    is_flag=True,
    default=False,
    help="Follow a running build's log",
)
@update_ctx
@pass_logger
@pass_config
def cmd_build_logs_tail(
    config: UserConfig,
    logger: logging.Logger,
    build_id: int,
    num_msgs: int | None,
    probe_frequency: float,
    follow: bool,
) -> None:
    last_id: str | None = None
    if follow:
        click.echo(
            f"--- follow build {build_id}, freq: {probe_frequency}, n: {num_msgs}"
        )
    else:
        click.echo(f"--- tail build {build_id}, n: {num_msgs}")

    def _out_msgs(msgs: list[str]) -> None:
        for msg in msgs:
            click.echo(f"{msg.strip()}")

    if not follow:
        try:
            res = _tail_builds_log(logger, config, build_id, max_msgs=num_msgs)
        except CBCError as e:
            click.echo(f"error tailing build '{build_id}' log: {e}", err=True)
            sys.exit(errno.ENOTRECOVERABLE)

        _out_msgs(res.msgs)
        click.echo("--- log tail end ---")
        return

    while True:
        try:
            res = _follow_builds_log(
                logger, config, build_id, since=last_id, max_msgs=num_msgs
            )
        except CBCError as e:
            click.echo(f"error following build '{build_id}' log: {e}", err=True)
            sys.exit(errno.ENOTRECOVERABLE)

        _out_msgs(res.msgs)

        last_id = res.last_id
        if res.end_of_stream:
            click.echo("--- log stream ended ---")
            break

        time.sleep(probe_frequency)


@cmd_build_logs_grp.command("get", help="Tail a build's log")
@click.argument("build_id", type=int, metavar="ID", required=True)
@click.option(
    "-o",
    "--output",
    "output_path",
    type=click.Path(path_type=Path, dir_okay=False, file_okay=True),
    required=False,
    help="Destination file",
)
@update_ctx
@pass_logger
@pass_config
def cmd_build_logs_get(
    config: UserConfig,
    logger: logging.Logger,
    build_id: int,
    output_path: Path | None,
) -> None:
    try:
        dpath = _download_log_file(logger, config, build_id, dest_path=output_path)
    except CBCError as e:
        click.echo(f"error downloading log file for build {build_id}: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"log file written to '{dpath}'")
