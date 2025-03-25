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
from pathlib import Path
import random
import string
from typing import override

from ceslib.errors import CESError
from ceslib.logger import log as root_logger
from ceslib.versions.desc import VersionDescriptor
from ceslib.versions.errors import NoSuchVersionDescriptorError
from ceslib.utils.podman import PodmanError, podman_run


log = root_logger.getChild("runner")


class RunnerError(CESError):
    @override
    def __str__(self) -> str:
        return f"Runner error: {self.msg}"


async def runner(
    desc_file_path: Path,
    tools_path: Path,
    secrets_file_path: Path,
    scratch_path: Path,
    scratch_container_path: Path,
    components_path: Path,
    containers_path: Path,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
    *,
    ccache_path: Path | None = None,
    timeout: float | None = None,
    upload: bool = True,
    skip_build: bool = False,
    force: bool = False,
) -> None:
    log.info(f"""run the runner:
    desc file path:          {desc_file_path}
    tools path:              {tools_path}
    secrets file path:       {secrets_file_path}
    scratch path:            {scratch_path}
    scratch containers path: {scratch_container_path}
    components path:         {components_path}
    containers path:         {containers_path}
    ccache path:             {ccache_path}
    vault: addr = {vault_addr}, role id = {vault_role_id}, transit = {vault_transit}
    timeout: {timeout}
    upload: {upload}, skip_build: {skip_build}, force: {force}

""")

    if not desc_file_path.exists():
        log.error(f"version descriptor does not exist at '{desc_file_path}'")
        raise NoSuchVersionDescriptorError(desc_file_path)

    try:
        desc = VersionDescriptor.read(desc_file_path)
    except CESError as e:
        msg = f"error loading version descriptor: {e}"
        log.error(e)
        raise RunnerError(msg)

    desc_mount_loc = f"/runner/{desc_file_path.name}"

    podman_volumes = {
        desc_file_path.resolve().as_posix(): desc_mount_loc,
        tools_path.resolve().as_posix(): "/runner/tools",
        secrets_file_path.resolve().as_posix(): "/runner/secrets.json",
        scratch_path.resolve().as_posix(): "/runner/scratch",
        scratch_container_path.resolve().as_posix(): "/var/lib/containers:Z",
        components_path.resolve().as_posix(): "/runner/components",
        containers_path.resolve().as_posix(): "/runner/containers",
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

    ctr_name = "cbs_" + "".join(random.choices(string.ascii_lowercase, k=10))

    try:
        rc, _, stderr = await podman_run(
            image=desc.distro,
            env={
                "VAULT_ADDR": vault_addr,
                "VAULT_ROLE_ID": vault_role_id,
                "VAULT_SECRET_ID": vault_secret_id,
                "VAULT_TRANSIT": vault_transit,
                "CBS_DEBUG": "1" if log.getEffectiveLevel() == logging.DEBUG else "0",
            },
            args=podman_args,
            volumes=podman_volumes,
            devices={"/dev/fuse": "/dev/fuse:rw"},
            entrypoint="/runner/tools/cbs-runner-entrypoint.sh",
            name=ctr_name,
            use_user_ns=False,
            timeout=timeout,
            use_host_network=True,
            unconfined=True,
        )
    except PodmanError as e:
        msg = f"error running build: {e}"
        log.error(msg)
        raise RunnerError(msg)
    except Exception as e:
        msg = f"unknown error running build: {e}"
        log.error(msg)
        raise RunnerError(msg)

    if rc != 0:
        msg = f"error running build (rc={rc}): {stderr}"
        log.error(msg)
        raise RunnerError(msg)
