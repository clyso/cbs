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

import abc
import asyncio
import os
import re
import shutil
import subprocess
from asyncio.streams import StreamReader
from collections.abc import Callable, Coroutine
from io import StringIO
from pathlib import Path
from typing import Any, override

from cbscore.errors import CESError
from cbscore.logger import logger as root_logger

logger = root_logger.getChild("utils")


class CommandError(CESError):
    @override
    def __str__(self) -> str:
        return "Command error" + f": {self.msg}" if self.msg else ""


class SecureArg(abc.ABC):
    @property
    @abc.abstractmethod
    def value(self) -> str:
        pass


class Password(SecureArg):
    _value: str

    def __init__(self, value: str) -> None:
        super().__init__()
        self._value = value

    @override
    def __str__(self) -> str:
        return "<CENSORED>"

    @override
    def __repr__(self) -> str:
        return "Password(<CENSORED>)"

    @property
    @override
    def value(self) -> str:
        return self._value


class PasswordArg(SecureArg):
    arg: str
    password: Password

    def __init__(self, arg: str, value: str) -> None:
        super().__init__()
        self.arg = arg
        self.password = Password(value)

    @override
    def __str__(self) -> str:
        return f"{self.arg}={self.password}"

    @property
    @override
    def value(self) -> str:
        return f"{self.arg}={self.password.value}"


class SecureURL(SecureArg):
    _url: str
    _args: dict[str, str | SecureArg]

    def __init__(self, _url: str, **kwargs: str | SecureArg) -> None:
        super().__init__()
        self._url = _url
        self._args = kwargs

    @override
    def __str__(self) -> str:
        return self._url.format(**self._args)

    @override
    def __repr__(self) -> str:
        return f"SecureURL({self._url!s})"

    @property
    @override
    def value(self) -> str:
        _args = {name: self._get_value(arg) for name, arg in self._args.items()}
        return self._url.format(**_args)

    def _get_value(self, v: str | SecureArg) -> str:
        return v if isinstance(v, str) else v.value


MaybeSecure = str | SecureArg
CmdArgs = list[MaybeSecure]


def get_maybe_secure_arg(value: MaybeSecure) -> str:
    return value if isinstance(value, str) else value.value


def _sanitize_cmd(cmd: CmdArgs) -> list[str]:
    sub_pattern = re.compile(r"(.*)(?:(--pass(?:phrase)[\s=]+)[^\s]+)")

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
        sanitized.append(s)

    return sanitized


def get_unsecured_cmd(orig: CmdArgs) -> list[str]:
    cmd: list[str] = []
    for c in orig:
        if isinstance(c, SecureArg):
            cmd.append(c.value)
        else:
            cmd.append(c)
    return cmd


def run_cmd(cmd: CmdArgs, env: dict[str, str] | None = None) -> tuple[int, str, str]:
    logger.debug(f"sync run '{_sanitize_cmd(cmd)}'")
    try:
        p = subprocess.run(get_unsecured_cmd(cmd), env=env, capture_output=True)  # noqa: S603
    except OSError as e:
        logger.exception(f"error running '{_sanitize_cmd(cmd)}'")
        raise CESError() from e

    if p.returncode != 0:
        logger.error(
            f"error running '{_sanitize_cmd(cmd)}': "
            + f"retcode = {p.returncode}, res: {p.stderr}"
        )
        return (p.returncode, "", p.stderr.decode("utf-8"))

    return (0, p.stdout.decode("utf-8"), p.stderr.decode("utf-8"))


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


AsyncRunCmdOutCallback = Callable[[str], Coroutine[Any, Any, None]]  # pyright: ignore[reportExplicitAny]


async def async_run_cmd(
    cmd: CmdArgs,
    *,
    outcb: AsyncRunCmdOutCallback | None = None,
    timeout: float | None = None,
    cwd: Path | None = None,
    reset_python_env: bool = False,
    extra_env: dict[str, str] | None = None,
) -> tuple[int, str, str]:
    logger.debug(f"async run '{_sanitize_cmd(cmd)}'")

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
