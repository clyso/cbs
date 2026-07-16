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

import subprocess
from typing import override

from cbscommon.process.cmds import get_unsecured_cmd, sanitize_cmd
from cbscommon.process.types import (
    CmdArgs,
    MaybeSecure,
    SecureArg,
)

from cbscore.errors import CESError
from cbscore.logger import logger as root_logger

logger = root_logger.getChild("utils")


class CommandError(CESError):
    @override
    def __str__(self) -> str:
        return "Command error" + f": {self.msg}" if self.msg else ""


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


def get_maybe_secure_arg(value: MaybeSecure) -> str:
    return value if isinstance(value, str) else value.value


def run_cmd(cmd: CmdArgs, env: dict[str, str] | None = None) -> tuple[int, str, str]:
    logger.debug(f"sync run '{sanitize_cmd(cmd)}'")
    try:
        p = subprocess.run(get_unsecured_cmd(cmd), env=env, capture_output=True)  # noqa: S603
    except OSError as e:
        logger.exception(f"error running '{sanitize_cmd(cmd)}'")
        raise CESError() from e

    if p.returncode != 0:
        logger.error(
            f"error running '{sanitize_cmd(cmd)}': "
            + f"retcode = {p.returncode}, res: {p.stderr}"
        )
        return (p.returncode, "", p.stderr.decode("utf-8"))

    return (0, p.stdout.decode("utf-8"), p.stderr.decode("utf-8"))
