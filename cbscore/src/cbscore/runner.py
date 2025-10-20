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

from cbscore.errors import CESError
from cbscore.logger import logger as root_logger
from cbscore.utils.podman import PodmanError, podman_run, podman_stop
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
    secrets_path: Path,
    scratch_path: Path,
    scratch_containers_path: Path,
    components_paths: list[Path],
    vault_config_path: Path,
    *,
    run_name: str | None = None,
    ccache_path: Path | None = None,
    entrypoint_path: Path | None = None,
    timeout: float | None = None,
    upload: bool = True,
    skip_build: bool = False,
    force: bool = False,
) -> None:
    our_actual_loc = Path(__file__).parent

    entrypoint_path = (
        entrypoint_path
        if entrypoint_path
        else our_actual_loc / "_tools" / "cbscore-entrypoint.sh"
    ).resolve()

    logger.info(f"""run the runner:
    desc file path:          {desc_file_path}
    cbscore path:            {cbscore_path}
    entrypoint:              {entrypoint_path}
    secrets file path:       {secrets_path}
    scratch path:            {scratch_path}
    scratch containers path: {scratch_containers_path}
    components paths:        {components_paths}
    ccache path:             {ccache_path}
    vault config path:       {vault_config_path}
    timeout: {timeout}
    upload: {upload}, skip_build: {skip_build}, force: {force}

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
        logger.exception(msg)
        raise RunnerError(msg) from e

    desc_mount_loc = f"/runner/{desc_file_path.name}"

    # propagate exception
    components_path = _setup_components_dir(components_paths)
    logger.debug(f"components contents: {list(components_path.walk())}")

    podman_volumes = {
        desc_file_path.resolve().as_posix(): desc_mount_loc,
        cbscore_path.resolve().as_posix(): "/runner/cbscore",
        entrypoint_path.resolve().as_posix(): "/runner/entrypoint.sh",
        vault_config_path.resolve().as_posix(): "/runner/cbs-build.vault.json",
        secrets_path.resolve().as_posix(): "/runner/secrets.json",
        scratch_path.resolve().as_posix(): "/runner/scratch",
        scratch_containers_path.resolve().as_posix(): "/var/lib/containers:Z",
        components_path.resolve().as_posix(): "/runner/components",
    }

    podman_args = ["--desc", desc_mount_loc]

    if ccache_path:
        ccache_path_loc = ccache_path.resolve().as_posix()
        podman_volumes[ccache_path_loc] = "/runner/ccache"
        podman_args.extend(["--ccache-path", "/runner/ccache"])

    if skip_build:
        podman_args.append("--skip-build")

    if upload:
        podman_args.append("--upload")

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
