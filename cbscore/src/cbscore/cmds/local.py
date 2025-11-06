# cbsbuild - commands - local builds
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
import tempfile
from pathlib import Path

import click

from cbscore.cmds import Ctx, pass_ctx
from cbscore.config import Config
from cbscore.errors import MalformedVersionError
from cbscore.runner import RunnerError, runner
from cbscore.versions.create import version_create_helper
from cbscore.versions.errors import VersionError
from cbscore.versions.utils import parse_version


@click.group("local", help="Handle local builds.")
def cmd_local_grp() -> None:
    pass


@cmd_local_grp.command("build", help="Build locally.")
@click.argument("version", metavar="VERSION", type=str)
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
    "component_refs",
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
    "-n",
    "--image-name",
    type=str,
    help="Name for this release's image",
    required=False,
    metavar="NAME",
)
@click.option(
    "--image-tag",
    type=str,
    help="Tag for this release's image",
    required=False,
    metavar="TAG",
)
@click.option(
    "--username",
    "user_name",
    type=str,
    required=True,
    metavar="NAME",
    envvar=("USER", "USERNAME"),
    help="User's name to associate to the build.",
)
@click.option(
    "--cbscore-path",
    "cbscore_path",
    help="Path to the 'cbs' directory.",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
)
@click.option(
    "-e",
    "--cbs-entrypoint",
    "cbs_entrypoint_path",
    help="Path to the 'cbs' builder's entrypoint script.",
    type=click.Path(
        exists=True,
        dir_okay=False,
        file_okay=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=False,
)
@click.option(
    "--timeout",
    help="Specify how long the build should be allowed to take, in seconds.",
    type=float,
    required=False,
)
@pass_ctx
def cmd_local_build(
    ctx: Ctx,
    version: str,
    version_type: str,
    component_refs: tuple[str, ...],
    components_paths: tuple[Path, ...],
    component_uri_overrides: tuple[str, ...],
    distro: str,
    el_version: int,
    image_name: str,
    image_tag: str | None,
    user_name: str,
    cbscore_path: Path,
    cbs_entrypoint_path: Path | None,
    timeout: int,
) -> None:
    assert ctx.vault_config_path  # FIXME: this will go away
    assert ctx.config_path
    try:
        config = Config.load(ctx.config_path)
    except Exception as e:
        click.echo(f"error loading config from '{ctx.config_path}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        prefix, _, _, _, _ = parse_version(version)
    except MalformedVersionError:
        click.echo(f"malformed version '{version}'")
        sys.exit(errno.EINVAL)

    image_name = (
        image_name
        if image_name
        else ((f"{prefix}/" if prefix else "vanilla/") + version_type)
    )
    image_tag = image_tag if image_tag else version

    if cbs_entrypoint_path and not cbs_entrypoint_path.exists():
        click.echo(f"cbs entrypoint at '{cbs_entrypoint_path}' does not exist")
        sys.exit(errno.ENOENT)

    try:
        desc = version_create_helper(
            version,
            version_type,
            list(component_refs),
            list(components_paths),
            list(component_uri_overrides),
            distro,
            el_version,
            None,
            image_name,
            image_tag,
            user_name,
            "",
        )
    except VersionError as e:
        click.echo(f"error creating local version descriptor: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    print(f"run local build, desc:\n{desc.model_dump_json(indent=2)}")

    _, tmp_file = tempfile.mkstemp(prefix="cbscore-", text=True)
    tmp_file_path = Path(tmp_file)
    try:
        _ = tmp_file_path.write_text(desc.model_dump_json(indent=2))
    except Exception as e:
        click.echo(f"unable to write desc file to '{tmp_file_path}': {e}")
        tmp_file_path.unlink(missing_ok=True)
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(
            runner(
                tmp_file_path,
                cbscore_path,
                config.secrets_path,
                config.scratch_path,
                config.scratch_containers_path,
                config.components_path,
                ctx.vault_config_path,
                ccache_path=config.ccache_path,
                entrypoint_path=cbs_entrypoint_path,
                timeout=timeout,
                upload=False,
                skip_build=False,
                force=False,
            )
        )
    except (RunnerError, Exception) as e:
        click.echo(f"error running build: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    finally:
        tmp_file_path.unlink()

    click.echo("build completed")
