# CES library - utilities
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
import os
import re
import shutil
import subprocess
from io import StringIO
from pathlib import Path
from typing import Callable

from ceslib.errors import CESError
from ceslib.logging import log as root_logger

log = root_logger.getChild("utils")


class CommandError(CESError):
    pass


def _sanitize_cmd(cmd: list[str]) -> list[str]:
    sub_pattern = re.compile(r"(.*)(?:(--pass(?:phrase)[\s=]+)[^\s]+)")

    sanitized: list[str] = []
    next_secure = False
    for c in cmd:
        if c == "--passphrase" or c == "--pass":
            next_secure = True
            sanitized.append(c)
            continue

        if next_secure:
            sanitized.append("****")
            next_secure = False
            continue

        s = re.sub(sub_pattern, r"\1\2****", c)
        sanitized.append(s)

    return sanitized


def run_cmd(cmd: list[str], env: dict[str, str] | None = None) -> tuple[int, str, str]:
    log.debug(f"sync run '{_sanitize_cmd(cmd)}'")
    try:
        p = subprocess.run(cmd, env=env, capture_output=True)
    except OSError as e:
        log.error(f"error running '{_sanitize_cmd(cmd)}': {e}")
        raise CESError()

    if p.returncode != 0:
        log.error(
            f"error running '{_sanitize_cmd(cmd)}': "
            + f"retcode = {p.returncode}, res: {p.stderr}"
        )
        return (p.returncode, "", p.stderr.decode("utf-8"))

    return (0, p.stdout.decode("utf-8"), p.stderr.decode("utf-8"))


def _reset_python_env(env: dict[str, str]) -> dict[str, str]:
    log.debug("reset python env for command")

    python3_loc = shutil.which("python3")
    if not python3_loc:
        print("python3 executable not found")
        return env

    log.debug(f"python3 location: {python3_loc}")

    python3_path = Path(python3_loc)
    if python3_path.parent.full_match("/usr/bin"):
        log.debug("nothing to do to python3 path")
        return env

    orig_path = env.get("PATH")
    assert orig_path
    paths = orig_path.split(":")

    new_paths: list[str] = []
    for p in paths:
        if Path(p).full_match(python3_path.parent):
            continue
        new_paths.append(p)

    env["PATH"] = ":".join(new_paths)
    return env


async def async_run_cmd(
    cmd: list[str],
    *,
    outcb: Callable[[str], None] | None = None,
    timeout: float = 2 * 60 * 60,  # 2h in seconds, because why not.
    cwd: Path | None = None,
    reset_python_env: bool = False,
    extra_env: dict[str, str] | None = None,
) -> tuple[int, str, str]:
    log.debug(f"async run '{_sanitize_cmd(cmd)}'")

    env: dict[str, str] = os.environ.copy()
    if reset_python_env:
        env = _reset_python_env(env)

    if extra_env:
        env.update(extra_env)

    p = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        cwd=cwd,
        env=env,
    )
    assert p.stdout
    assert p.stderr

    log.debug(f"env path: {os.environ['PATH']}")

    try:
        stdout, stderr = await asyncio.gather(
            _tee(p.stdout, outcb), _tee(p.stderr, outcb)
        )
        retcode = await asyncio.wait_for(p.wait(), timeout)
    except asyncio.TimeoutError:
        # attempt to kill the process, if possible. Some states may prevent it from being killed.
        p.kill()
        log.error(f"running exceeded timeout ({timeout} secs)")
        raise CommandError("timeout exceeded")

    return retcode, stdout, stderr


async def _tee(
    reader: asyncio.StreamReader, cb: Callable[[str], None] | None = None
) -> str:
    collected = StringIO()
    async for line in reader:
        ln = line.decode("utf-8")
        _ = collected.write(ln)
        if cb is not None:
            cb(ln.rstrip())

    return collected.getvalue()
