#!/usr/bin/env python3

# Builds a declarative version, using a container
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
import os
import sys
from collections.abc import Callable
from functools import update_wrapper
from pathlib import Path
from typing import Concatenate, ParamSpec, TypeVar, cast

import click

from cbscore.builder.builder import Builder
from cbscore.config import Config, VaultAppRoleConfig, VaultConfig, VaultUserPassConfig
from cbscore.errors import CESError
from cbscore.logger import logger as root_logger
from cbscore.runner import RunnerError, runner
from cbscore.versions.desc import VersionDescriptor

logger = root_logger.getChild("main")


class Ctx:
    config_path: Path | None = None
    vault_config_path: Path | None = None


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)

_R = TypeVar("_R")
_T = TypeVar("_T")
_P = ParamSpec("_P")


def with_config(f: Callable[Concatenate[Config, _P], _R]) -> Callable[_P, _R]:
    """Pass the CES patches repo path from the context to the function."""

    def inner(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            logger.error(f"missing context for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)
        if not ctx.config_path:
            logger.error("configuration file path not provided")
            sys.exit(errno.EINVAL)

        try:
            config = Config.load(ctx.config_path)
        except Exception as e:
            logger.error(f"unable to read configuration file: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        return f(config, *args, **kwargs)

    return update_wrapper(inner, f)


@click.group()
@click.option(
    "-d", "--debug", help="Enable debug output", is_flag=True, envvar="CBS_DEBUG"
)
@click.option(
    "-c",
    "--config",
    "config_path",
    help="Path to configuration file.",
    type=click.Path(
        exists=False,
        dir_okay=False,
        file_okay=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    default="cbs-build.config.json",
)
@click.option(
    "--vault",
    "vault_config_path",
    help="Path to Vault's configuration file.",
    type=click.Path(
        exists=False,
        dir_okay=False,
        file_okay=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    default="cbs-build.vault.json",
)
@pass_ctx
def main(ctx: Ctx, debug: bool, config_path: Path, vault_config_path: Path) -> None:
    if debug:
        root_logger.setLevel(logging.DEBUG)

    ctx.config_path = config_path
    ctx.vault_config_path = vault_config_path


@main.command("config-init", help="Initialize the configuration file.")
@click.option(
    "--vault-addr",
    "vault_addr",
    type=str,
    required=True,
    prompt="Vault address",
)
@click.option(
    "--vault-transit",
    "vault_transit",
    type=str,
    required=True,
    prompt="Vault transit",
)
@pass_ctx
def config_init(ctx: Ctx, vault_addr: str, vault_transit: str) -> None:
    assert ctx.config_path
    assert ctx.vault_config_path
    config_path = ctx.config_path
    vault_config_path = ctx.vault_config_path

    cwd = Path.cwd()

    vault_auth_user: VaultUserPassConfig | None = None
    vault_approle: VaultAppRoleConfig | None = None
    vault_token: str | None = None

    if click.confirm("Specify user/pass auth for vault?"):
        username = cast(str, click.prompt("Username", type=str))
        password = cast(str, click.prompt("Password", hide_input=True, type=str))
        vault_auth_user = VaultUserPassConfig(username=username, password=password)
    elif click.confirm("Specify AppRole auth for vault?"):
        role_id = cast(str, click.prompt("Role ID", type=str))
        secret_id = cast(str, click.prompt("Secret ID", type=str))
        vault_approle = VaultAppRoleConfig(role_id=role_id, secret_id=secret_id)
    else:
        token = cast(str | None, click.prompt("Vault token"))
        if not token:
            print("must provide a vault token")
            sys.exit(errno.EINVAL)
        vault_token = token

    vault_config = VaultConfig(
        vault_addr=vault_addr,
        vault_transit=vault_transit,
        auth_user=vault_auth_user,
        auth_approle=vault_approle,
        auth_token=vault_token,
    )

    components_paths: list[Path] = []
    default_components_path = cwd / "components"
    if default_components_path.exists() and click.confirm(
        f"Use '{default_components_path}' as components path?"
    ):
        components_paths.append(default_components_path)

    if click.confirm("Specify components paths?"):
        while True:
            comp_path = Path(
                cast(str, click.prompt("Components path", type=str))
            ).resolve()
            if not comp_path.exists() or not comp_path.is_dir():
                print(
                    f"provided path '{comp_path}' does not exist or is not a directory"
                )
            else:
                components_paths.append(comp_path)

            if not click.confirm("Add another components path?"):
                break

    default_secrets_paths = [
        cwd / "cbs" / "secrets.json",
        cwd / "_local" / "secrets.json",
        cwd / "secrets.json",
    ]
    secrets_path: Path | None = None
    for entry in default_secrets_paths:
        if entry.exists() and entry.is_file():
            secrets_path = entry
            break

    secrets_prompt_str = ""
    if not secrets_path:
        print("unable to find default 'secrets.json' file")
    else:
        print(f"using '{secrets_path}' as default 'secrets.json' file")
        secrets_prompt_str = "a different "

    if click.confirm(f"Specify {secrets_prompt_str}path to 'secrets.json'?"):
        secrets_path = Path(cast(str, click.prompt("Path to 'secrets.json'"))).resolve()

    if not secrets_path:
        print("path to 'secrets.json' not provided, abort")
        sys.exit(errno.EINVAL)

    scratch_path = Path(cast(str, click.prompt("Scratch path", type=str))).resolve()
    scratch_containers_path = Path(
        cast(str, click.prompt("Scratch containers path", type=str))
    ).resolve()
    ccache_path: Path | None = None
    if click.confirm("Specify ccache path?"):
        ccache_path = Path(cast(str, click.prompt("ccache path", type=str))).resolve()

    config = Config(
        components_path=components_paths,
        secrets_path=secrets_path,
        scratch_path=scratch_path,
        scratch_containers_path=scratch_containers_path,
        ccache_path=ccache_path,
    )

    if config_path.exists() and not click.confirm("Config file exists, overwrite?"):
        print(f"do not write config file to '{config_path}'")
        sys.exit(errno.ENOTRECOVERABLE)

    if vault_config_path.exists() and not click.confirm(
        "Vault config exists, overwrite?"
    ):
        print(f"do not write vault config to '{vault_config_path}'")
        sys.exit(errno.ENOTRECOVERABLE)

    print(f"""
==== vault ====
{vault_config.model_dump_json(indent=2)}

==== config ====
{config.model_dump_json(indent=2)}

""")

    if not click.confirm("Write config?"):
        print("do not write config files")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        _ = config_path.write_text(config.model_dump_json(indent=2))
        _ = vault_config_path.write_text(vault_config.model_dump_json(indent=2))
    except Exception as e:
        print(f"error writing config file: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    print(f"""
wrote config files

      config: {config_path}
vault config: {vault_config_path}
""")


@main.command("build", help="Start a containerized build.")
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
    "--cbs-path",
    "cbs_path",
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
def build(
    ctx: Ctx,
    desc_path: Path,
    cbs_path: Path,
    cbs_entrypoint_path: Path | None,
    upload: bool,
    timeout: float | None,
    skip_build: bool,
    force: bool,
) -> None:
    # our_dir = Path(sys.argv[0]).parent
    our_actual_loc = Path(__file__).parent

    entrypoint_path = (
        cbs_entrypoint_path
        if cbs_entrypoint_path
        else our_actual_loc / "_tools" / "cbscore-entrypoint.sh"
    ).resolve()
    if not entrypoint_path.exists() or not entrypoint_path.is_file():
        print(f"error: unable to find entrypoint script at '{entrypoint_path}'")
        sys.exit(errno.ENOENT)

    if entrypoint_path.is_symlink():
        print(f"error: entrypoint script at '{entrypoint_path}' can't be a symlink")
        sys.exit(errno.EINVAL)

    if not os.access(entrypoint_path, os.X_OK):
        print(f"error: entrypoint script at '{entrypoint_path}' is not executable")
        sys.exit(errno.EACCES)

    print(f"""
runner loc path: {our_actual_loc}
     entrypoint: {entrypoint_path}
       cbs path: {cbs_path}
""")

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
                cbs_path,
                entrypoint_path,
                config.secrets_path,
                config.scratch_path,
                config.scratch_containers_path,
                config.components_path,
                ctx.vault_config_path,
                ccache_path=config.ccache_path,
                timeout=timeout,
                upload=upload,
                skip_build=skip_build,
                force=force,
            )
        )
    except (RunnerError, Exception):
        logger.exception(f"error building '{desc_path}'")
        sys.exit(1)


@main.group("runner", help="Build Runner related operations")
def runner_grp() -> None:
    pass


@runner_grp.command(
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
def runner_build(
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

    try:
        loop = asyncio.new_event_loop()
        loop.run_until_complete(builder.run())
    except Exception:
        logger.exception("unable to run build")
        sys.exit(1)


if __name__ == "__main__":
    main()
