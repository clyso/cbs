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

from ceslib.errors import CESError

from . import async_run_cmd
from . import log as parent_logger

log = parent_logger.getChild("podman")


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
    use_user_ns: bool = False,
    timeout: float = 2 * 60 * 60,  # 2 hours, because why not.
    use_host_network: bool = False,
    unconfined: bool = False,
) -> tuple[int, str, str]:
    cmd = ["podman", "run", "--security-opt", "label=disable"]

    if use_user_ns:
        cmd.extend(["--userns", "keep-id"])

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
        log.debug(s)

    rc, stdout, stderr = await async_run_cmd(cmd, timeout=timeout, outcb=cb)
    if rc != 0:
        log.error(f"running podman: {stderr} ({rc})")
    return rc, stdout, stderr
