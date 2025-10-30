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
from pathlib import Path

import click

from cbscore.builder import BuilderError
from cbscore.builder.builder import Builder
from cbscore.cmds import Ctx, pass_ctx
from cbscore.cmds import logger as parent_logger
from cbscore.config import Config, VaultConfig
from cbscore.errors import CESError
from cbscore.runner import RunnerError, runner
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
    "--upload/--no-upload",
    help="Upload artifacts to Clyso's S3",
    is_flag=True,
    default=True,
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
@pass_ctx
def cmd_build(
    ctx: Ctx,
    desc_path: Path,
    cbscore_path: Path,
    cbs_entrypoint_path: Path | None,
    upload: bool,
    timeout: float | None,
    skip_build: bool,
    force: bool,
) -> None:
    assert ctx.config_path
    assert ctx.vault_config_path

    try:
        config = Config.load(ctx.config_path)
    except Exception as e:
        print(f"error loading config from '{ctx.config_path}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(
            runner(
                desc_path,
                cbscore_path,
                config.secrets_path,
                config.scratch_path,
                config.scratch_containers_path,
                config.components_path,
                ctx.vault_config_path,
                ccache_path=config.ccache_path,
                entrypoint_path=cbs_entrypoint_path,
                timeout=timeout,
                upload=upload,
                skip_build=skip_build,
                force=force,
            )
        )
    except (RunnerError, Exception):
        logger.exception(f"error building '{desc_path}'")
        sys.exit(1)


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
    "components_path",
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
@pass_ctx
def cmd_runner_build(
    ctx: Ctx,
    desc_path: Path,
    scratch_dir: Path,
    components_path: Path,
    secrets_path: Path,
    ccache_path: Path | None,
    upload: bool,
    skip_build: bool,
    force: bool,
) -> None:
    logger.debug(f"desc: {desc_path}")
    logger.debug(f"vault config: {ctx.vault_config_path}")
    logger.debug(f"scratch dir: {scratch_dir}")
    logger.debug(f"secrets path: {secrets_path}")
    logger.debug(f"components path: {components_path}")
    logger.debug(f"ccache path: {ccache_path}")
    logger.debug(f"upload: {upload}")
    logger.debug(f"skip_build: {skip_build}")
    logger.debug(f"force: {force}")

    if not desc_path.exists():
        logger.error(f"build descriptor does not exist at '{desc_path}'")
        sys.exit(errno.ENOENT)

    if not ctx.vault_config_path or not ctx.vault_config_path.exists():
        logger.error("vault config path not provided or does not exist")
        sys.exit(errno.ENOENT)

    try:
        desc = VersionDescriptor.read(desc_path)
    except CESError:
        logger.exception("unable to read descriptor")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        vault_config = VaultConfig.load(ctx.vault_config_path)
    except Exception as e:
        logger.error(f"unable to read vault config from '{ctx.vault_config_path}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    print(f"run builder, desc = '{desc_path}'")
    try:
        builder = Builder(
            desc,
            vault_config,
            scratch_dir,
            secrets_path,
            components_path,
            upload=upload,
            ccache_path=ccache_path,
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
