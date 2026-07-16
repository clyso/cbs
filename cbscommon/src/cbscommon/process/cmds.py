# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH

import asyncio
import os
import re
import shutil
from asyncio.streams import StreamReader
from io import StringIO
from pathlib import Path
from typing import IO, Any

from cbscommon.process.types import AsyncRunCmdOutCallback, CmdArgs, SecureArg

from . import logger


def sanitize_cmd(cmd: CmdArgs) -> list[str]:
    sub_pattern = re.compile(r"(.*)(?:(--pass(?:phrase)[\s=]+)[^\s]+)")
    gh_token_pattern = re.compile("crt:[^@]*")
    sanitized: list[str] = []
    next_secure = False
    for c in cmd:
        if isinstance(c, SecureArg):
            sanitized.append(str(c))
            continue

        if c == "--passphrase" or c == "--pass":
            next_secure = True
            sanitized.append(c)
            continue

        if next_secure:
            sanitized.append("****")
            next_secure = False
            continue

        s = re.sub(sub_pattern, r"\1\2****", c)
        s = re.sub(gh_token_pattern, "crt:token", s)
        sanitized.append(s)

    return sanitized


def _reset_python_env(env: dict[str, str]) -> dict[str, str]:
    logger.debug("reset python env for command")

    python3_loc = shutil.which("python3")
    if not python3_loc:
        print("python3 executable not found")
        return env

    logger.debug(f"python3 location: {python3_loc}")

    python3_path = Path(python3_loc)
    if python3_path.parent.full_match("/usr/bin"):
        logger.debug("nothing to do to python3 path")
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


def get_unsecured_cmd(orig: CmdArgs) -> list[str]:
    cmd: list[str] = []
    for c in orig:
        if isinstance(c, SecureArg):
            cmd.append(c.value)
        else:
            cmd.append(c)
    return cmd


async def async_run_cmd(
    cmd: CmdArgs,
    *,
    outcb: AsyncRunCmdOutCallback | None = None,
    timeout: float | None = None,
    cwd: Path | None = None,
    reset_python_env: bool = False,
    extra_env: dict[str, str] | None = None,
    stdin: int | IO[Any] | None = None,  # pyright: ignore[reportExplicitAny]
) -> tuple[int, str, str]:
    logger.debug(f"async run '{sanitize_cmd(cmd)}'")

    env: dict[str, str] = os.environ.copy()
    if reset_python_env:
        env = _reset_python_env(env)

    if extra_env:
        env.update(extra_env)

    logger.debug(f"run async subprocess, cwd: {cwd}, cmd: {cmd}")
    p = await asyncio.create_subprocess_exec(
        *(get_unsecured_cmd(cmd)),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        stdin=stdin,
        cwd=cwd,
        env=env,
    )

    async def read_stream(stream: StreamReader | None) -> str:
        collected = StringIO()

        if not stream:
            return ""

        async for line in stream:
            ln = line.decode("utf-8")
            if outcb:
                await outcb(ln)
            else:
                _ = collected.write(ln)

        return collected.getvalue()

    async def monitor():
        return await asyncio.gather(
            read_stream(p.stdout),
            read_stream(p.stderr),
        )

    try:
        retcode, (stdout, stderr) = await asyncio.wait_for(
            asyncio.gather(p.wait(), monitor()), timeout=timeout
        )
    except (TimeoutError, asyncio.CancelledError):
        # FIXME: evaluate all callers for whether they are properly handling these
        # exceptions, vs handling  a 'CommandError'. Or implement specific
        # 'CommandError' exceptions for timeout and cancellation.
        logger.error("async subprocess timed out or was cancelled")
        p.kill()
        _ = await p.wait()
        raise

    return retcode, stdout, stderr
