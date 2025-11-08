# cbsbuild - commands - builds
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

from cbscore.builder import BuilderError
from cbscore.builder.builder import Builder
from cbscore.cmds import Ctx, pass_ctx, with_config
from cbscore.cmds import logger as parent_logger
from cbscore.config import Config, ConfigError
from cbscore.errors import CESError
from cbscore.runner import RunnerError, runner
from cbscore.utils.secrets import SecretsError
from cbscore.versions.desc import VersionDescriptor

logger = parent_logger.getChild("builds")


@click.command("build", help="Start a containerized build.")
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
    help="Specify how long the build should be allowed to take",
    type=float,
    required=False,
)
@click.option(
    "--upload-to",
    "upload_to",
    help="Upload artifacts to specified storage.",
    type=str,
    required=False,
    metavar="HOST|ADDRESS",
)
@click.option(
    "--sign-with-gpg-id",
    "sign_with_gpg_id",
    help="Sign artifacts with specified gpg secret id.",
    type=str,
    required=False,
    metavar="GPG_SECRET_ID",
)
@click.option(
    "--sign-with-transit",
    "sign_with_transit",
    help="Sign container images with specified vault transit secret id.",
    type=str,
    required=False,
    metavar="TRANSIT_SECRET_ID",
)
@click.option(
    "--registry",
    "registry",
    help="Push built container images to specified registry.",
    type=str,
    required=False,
    metavar="HOST|ADDRESS",
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
@pass_ctx
def cmd_build(
    ctx: Ctx,
    desc_path: Path,
    cbscore_path: Path,
    cbs_entrypoint_path: Path | None,
    timeout: float | None,
    upload_to: str | None,
    sign_with_gpg_id: str | None,
    sign_with_transit: str | None,
    registry: str | None,
    skip_build: bool,
    force: bool,
) -> None:
    assert ctx.config_path

    try:
        config = Config.load(ctx.config_path)
    except Exception as e:
        click.echo(f"error loading config from '{ctx.config_path}': {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        secrets = config.get_secrets()
    except ConfigError as e:
        click.echo(f"unable to obtain secrets from config: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    _, secrets_path_str = tempfile.mkstemp(prefix="cbs-build-", suffix=".secrets.yaml")
    secrets_path = Path(secrets_path_str)
    try:
        secrets.store(secrets_path)
    except SecretsError as e:
        click.echo(f"unable to store secrets to temp '{secrets_path}': {e}")
        secrets_path.unlink()
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(
            runner(
                desc_path,
                cbscore_path,
                config,
                entrypoint_path=cbs_entrypoint_path,
                timeout=timeout,
                upload_to=upload_to,
                sign_with_gpg=sign_with_gpg_id,
                sign_with_transit=sign_with_transit,
                registry=registry,
                skip_build=skip_build,
                force=force,
            )
        )
    except (RunnerError, Exception):
        logger.exception(f"error building '{desc_path}'")
        sys.exit(1)
    finally:
        secrets_path.unlink()


@click.group("runner", help="Build Runner related operations.", hidden=True)
def cmd_runner_grp() -> None:
    pass


@cmd_runner_grp.command(
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
    "--upload-to",
    "upload_to",
    help="Upload artifacts to specified storage.",
    type=str,
    required=False,
    metavar="HOST|ADDRESS",
)
@click.option(
    "--sign-with-gpg-id",
    "sign_with_gpg_id",
    help="Sign artifacts with specified gpg secret id.",
    type=str,
    required=False,
    metavar="GPG_SECRET_ID",
)
@click.option(
    "--sign-with-transit",
    "sign_with_transit",
    help="Sign container images with specified vault transit secret id.",
    type=str,
    required=False,
    metavar="TRANSIT_SECRET_ID",
)
@click.option(
    "--registry",
    "registry",
    help="Push built container images to specified registry.",
    type=str,
    required=False,
    metavar="HOST|ADDRESS",
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
@with_config
def cmd_runner_build(
    config: Config,
    desc_path: Path,
    upload_to: str | None,
    sign_with_gpg_id: str | None,
    sign_with_transit: str | None,
    registry: str | None,
    skip_build: bool,
    force: bool,
) -> None:
    upload_to = (
        upload_to or config.secrets_config.storage if config.secrets_config else None
    )
    sign_with_gpg_id = (
        sign_with_gpg_id or config.secrets_config.gpg_signing
        if config.secrets_config
        else None
    )
    sign_with_transit = (
        sign_with_transit or config.secrets_config.transit_signing
        if config.secrets_config
        else None
    )
    registry = (
        registry or config.secrets_config.registry if config.secrets_config else None
    )

    logger.debug(f"""runner build called with:
   desc file path: {desc_path}
      scratch dir: {config.paths.scratch}
  components path: {config.paths.components}
     secrets path: {config.secrets}
      ccache path: {config.paths.ccache}
        upload to: {upload_to}
    sign with gpg: {sign_with_gpg_id}
sign with transit: {sign_with_transit}
         registry: {registry}
       skip build: {skip_build}
            force: {force}
""")

    if not desc_path.exists():
        logger.error(f"build descriptor does not exist at '{desc_path}'")
        sys.exit(errno.ENOENT)

    try:
        desc = VersionDescriptor.read(desc_path)
    except CESError:
        logger.exception("unable to read descriptor")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        builder = Builder(
            desc,
            config,
            upload_to=upload_to,
            sign_with_gpg=sign_with_gpg_id,
            sign_with_transit=sign_with_transit,
            registry=registry,
            skip_build=skip_build,
            force=force,
        )
    except BuilderError as e:
        logger.error(f"unable to initialize builder: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(builder.run())
    except Exception as e:
        logger.error(f"unable to run build: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
