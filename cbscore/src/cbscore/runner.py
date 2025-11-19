# CES library - runner
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

import logging
import os
import random
import shutil
import string
import tempfile
from pathlib import Path
from typing import override

from cbscore.config import Config, ConfigError
from cbscore.errors import CESError
from cbscore.logger import logger as root_logger
from cbscore.utils.podman import PodmanError, podman_run, podman_stop
from cbscore.utils.secrets import SecretsError
from cbscore.versions.desc import VersionDescriptor
from cbscore.versions.errors import NoSuchVersionDescriptorError

logger = root_logger.getChild("runner")


class RunnerError(CESError):
    @override
    def __str__(self) -> str:
        return f"Runner error: {self.msg}"


def gen_run_name(prefix: str = "ces_") -> str:
    return prefix + "".join(random.choices(string.ascii_lowercase, k=10))  # noqa: S311


def _setup_components_dir(components_paths: list[Path]) -> Path:
    # build a temporary directory for an aggregated components dir
    dst_path = Path(tempfile.mkdtemp(suffix=".cbs", prefix="components-"))
    for comp_dir in components_paths:
        for d in comp_dir.iterdir():
            if not d.is_dir():
                continue
            dest = dst_path / d.name
            try:
                _ = shutil.copytree(d, dest, dirs_exist_ok=True)
            except Exception as e:
                msg = f"unable to copy component '{d}' to '{dest}': {e}"
                logger.error(msg)
                raise RunnerError(msg) from e

    logger.debug(f"Using temporary components dir at '{dst_path}'")
    return dst_path


def _cleanup_components_dir(components_path: Path) -> None:
    try:
        shutil.rmtree(components_path, ignore_errors=True)
    except Exception as e:
        msg = f"unable to remove temporary components dir '{components_path}': {e}"
        logger.error(msg)
        raise RunnerError(msg) from e


async def runner(
    desc_file_path: Path,
    cbscore_path: Path,
    config: Config,
    *,
    run_name: str | None = None,
    replace_run: bool = False,
    entrypoint_path: Path | None = None,
    timeout: float | None = None,
    skip_build: bool = False,
    force: bool = False,
) -> None:
    our_actual_loc = Path(__file__).parent

    entrypoint_path = (
        entrypoint_path
        if entrypoint_path
        else our_actual_loc / "_tools" / "cbscore-entrypoint.sh"
    ).resolve()

    vault_config_path_str = f"{config.vault}" if config.vault else "not using vault"
    timeout_str = f"{timeout} seconds" if timeout else "no timeout"
    upload_to_str = (
        config.secrets_config.storage
        if config.secrets_config is not None
        and config.secrets_config.storage is not None
        else "not uploading"
    )
    sign_with_gpg_str = (
        config.secrets_config.gpg_signing
        if config.secrets_config is not None
        and config.secrets_config.gpg_signing is not None
        else "not gpg signing"
    )
    sign_with_transit_str = (
        config.secrets_config.transit_signing
        if config.secrets_config is not None
        and config.secrets_config.transit_signing is not None
        else "not transit signing"
    )
    registry_str = (
        config.secrets_config.registry
        if config.secrets_config is not None
        and config.secrets_config.registry is not None
        else "not pushing to registry"
    )

    secrets_files_str = ", ".join([p.as_posix() for p in config.secrets])
    component_paths_str = ", ".join([p.as_posix() for p in config.paths.components])
    ccache_path_str = (
        config.paths.ccache.as_posix() if config.paths.ccache else "not using ccache"
    )

    logger.info(f"""run the runner:
    desc file path:          {desc_file_path}
    cbscore path:            {cbscore_path}
    entrypoint:              {entrypoint_path}
    secrets file path:       {secrets_files_str}
    scratch path:            {config.paths.scratch}
    scratch containers path: {config.paths.scratch_containers}
    components paths:        {component_paths_str}
    ccache path:             {ccache_path_str}
    vault config path:       {vault_config_path_str}
    timeout:                 {timeout_str}
    upload to:               {upload_to_str}
    sign with gpg:           {sign_with_gpg_str}
    sign with transit:       {sign_with_transit_str}
    registry:                {registry_str}
    skip build:              {skip_build}
    force:                   {force}
""")

    if not entrypoint_path.exists() or not entrypoint_path.is_file():
        msg = f"error: unable to find entrypoint script at '{entrypoint_path}'"
        logger.error(msg)
        raise RunnerError(msg)

    if entrypoint_path.is_symlink():
        msg = f"error: entrypoint script at '{entrypoint_path}' can't be a symlink"
        logger.error(msg)
        raise RunnerError(msg)

    if not os.access(entrypoint_path, os.X_OK):
        msg = f"error: entrypoint script at '{entrypoint_path}' is not executable"
        logger.error(msg)
        raise RunnerError(msg)

    if not desc_file_path.exists():
        logger.error(f"version descriptor does not exist at '{desc_file_path}'")
        raise NoSuchVersionDescriptorError(desc_file_path)

    try:
        desc = VersionDescriptor.read(desc_file_path)
    except CESError as e:
        msg = f"error loading version descriptor: {e}"
        logger.error(msg)
        raise RunnerError(msg) from e

    desc_mount_loc = f"/runner/{desc_file_path.name}"

    # propagate exception
    components_path = _setup_components_dir(config.paths.components)
    logger.debug(f"components contents: {list(components_path.walk())}")

    # create temp file holding the secrets
    #
    _, secrets_tmp_file = tempfile.mkstemp(suffix="secrets.yaml", prefix="cbs-build-")
    secrets_tmp_path = Path(secrets_tmp_file)
    try:
        secrets = config.get_secrets()
        secrets.store(secrets_tmp_path)
    except (ConfigError, SecretsError) as e:
        secrets_tmp_path.unlink(missing_ok=True)
        msg = f"error creating temporary secrets file: {e}"
        logger.error(msg)
        raise RunnerError(msg) from e

    # new config to pass to the container, with adjusted paths.
    #
    new_config = config.model_copy(deep=True)
    new_config.secrets = [Path("/runner/cbs-build.secrets.yaml")]
    new_config.vault = Path("/runner/cbs-build.vault.yaml") if config.vault else None
    new_config.paths.scratch = Path("/runner/scratch")
    new_config.paths.scratch_containers = Path("/var/lib/containers")
    new_config.paths.components = [Path("/runner/components")]
    new_config.paths.ccache = Path("/runner/ccache") if config.paths.ccache else None
    _, new_config_tmp_file = tempfile.mkstemp(
        suffix=".config.yaml", prefix="cbs-build-"
    )
    new_config_path = Path(new_config_tmp_file)
    try:
        new_config.store(new_config_path)
    except ConfigError as e:
        secrets_tmp_path.unlink(missing_ok=True)
        new_config_path.unlink(missing_ok=True)
        msg = f"error creating temporary config file: {e}"
        logger.error(msg)
        raise RunnerError(msg) from e

    podman_args = ["--desc", desc_mount_loc]
    podman_volumes = {
        desc_file_path.resolve().as_posix(): desc_mount_loc,
        cbscore_path.resolve().as_posix(): "/runner/cbscore",
        entrypoint_path.resolve().as_posix(): "/runner/entrypoint.sh",
        new_config_path.resolve().as_posix(): "/runner/cbs-build.config.yaml",
        secrets_tmp_path.resolve().as_posix(): "/runner/cbs-build.secrets.yaml",
        config.paths.scratch.resolve().as_posix(): "/runner/scratch",
        config.paths.scratch_containers.resolve().as_posix(): "/var/lib/containers:Z",
        components_path.resolve().as_posix(): "/runner/components",
    }

    if config.vault:
        vault_config_path_loc = config.vault.resolve().as_posix()
        podman_volumes[vault_config_path_loc] = "/runner/cbs-build.vault.yaml"

    if config.paths.ccache:
        ccache_path_loc = config.paths.ccache.resolve().as_posix()
        podman_volumes[ccache_path_loc] = "/runner/ccache"

    if skip_build:
        podman_args.append("--skip-build")

    if force:
        podman_args.append("--force")

    ctr_name = run_name if run_name else gen_run_name()

    try:
        rc, _, stderr = await podman_run(
            image=desc.distro,
            env={
                "CBS_DEBUG": "1"
                if logger.getEffectiveLevel() == logging.DEBUG
                else "0",
            },
            args=podman_args,
            volumes=podman_volumes,
            devices={"/dev/fuse": "/dev/fuse:rw"},
            entrypoint="/runner/entrypoint.sh",
            name=ctr_name,
            use_user_ns=False,
            timeout=timeout,
            use_host_network=True,
            unconfined=True,
            replace_if_exists=replace_run,
        )
    except PodmanError as e:
        msg = f"error running build: {e}"
        logger.exception(msg)
        raise RunnerError(msg) from e
    except Exception as e:
        msg = f"unknown error running build: {e}"
        logger.exception(msg)
        raise RunnerError(msg) from e
    finally:
        _cleanup_components_dir(components_path)

    if rc != 0:
        msg = f"error running build (rc={rc}): {stderr}"
        logger.error(msg)
        raise RunnerError(msg)


async def stop(*, name: str | None = None, timeout: int = 1) -> None:
    """Stop the specified container (with `name`), or all containers on the host."""
    await podman_stop(name=name, timeout=timeout)
