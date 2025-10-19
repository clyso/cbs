# CES library - podman utilities
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

from typing import override

from cbscore.errors import CESError

from . import CmdArgs, async_run_cmd
from . import logger as parent_logger

logger = parent_logger.getChild("podman")


class PodmanError(CESError):
    retcode: int

    def __init__(self, retcode: int, msg: str) -> None:
        super().__init__(msg)
        self.retcode = retcode

    @override
    def __str__(self) -> str:
        return f"podman error: {self.msg} (retcode: {self.retcode})"


async def podman_run(
    image: str,
    *,
    args: list[str] | None = None,
    env: dict[str, str] | None = None,
    volumes: dict[str, str] | None = None,
    devices: dict[str, str] | None = None,
    entrypoint: str | None = None,
    name: str | None = None,
    use_user_ns: bool = False,
    timeout: float | None = None,
    use_host_network: bool = False,
    unconfined: bool = False,
) -> tuple[int, str, str]:
    cmd: CmdArgs = ["podman", "run", "--security-opt", "label=disable"]

    if name:
        cmd.extend(["--name", name])

    if use_user_ns:
        cmd.extend(["--userns", "keep-id"])

    if timeout:
        cmd.extend(["--timeout", str(int(timeout))])

    if unconfined:
        cmd.extend(["--security-opt", "seccomp=unconfined"])

    if env is not None:
        for k, v in env.items():
            cmd.extend(["--env", f"{k}={v}"])

    if volumes is not None:
        for src, dst in volumes.items():
            cmd.extend(["--volume", f"{src}:{dst}"])

    if devices:
        for src, dst in devices.items():
            cmd.extend(["--device", f"{src}:{dst}"])

    if use_host_network:
        cmd.extend(["--network", "host"])

    if entrypoint is not None:
        cmd.extend(["--entrypoint", entrypoint])

    cmd.append(image)
    if args is not None:
        cmd.extend(args)

    def cb(s: str) -> None:
        logger.debug(s)

    rc, stdout, stderr = await async_run_cmd(cmd, timeout=timeout, outcb=cb)
    if rc != 0:
        logger.error(f"running podman: {stderr} ({rc})")
    return rc, stdout, stderr


async def podman_stop(*, name: str | None = None, timeout: int = 1) -> None:
    """Stop either the specified container (with `name`) or all running containers."""
    cmd: CmdArgs = ["podman", "stop", "--time", str(timeout)]
    cmd.append(name if name else "--all")

    rc, _, stderr = await async_run_cmd(cmd)
    if rc != 0:
        logger.error(f"error stopping container: {stderr}")
