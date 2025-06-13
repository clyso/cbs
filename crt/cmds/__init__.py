# Ceph Release Tool - helps with managing and releasing Ceph versions
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

import errno
import logging
import sys
from collections.abc import Callable
from functools import update_wrapper
from pathlib import Path
from typing import Concatenate, ParamSpec, TypeVar

import click
from ceslib.utils.secrets import SecretsVaultMgr
from crtlib.db.db import ReleasesDB
from crtlib.db.errors import DBError
from crtlib.db.s3 import S3DB
from crtlib.errors import CRTError
from crtlib.logger import logger as parent_logger
from rich.console import Console
from rich.highlighter import RegexHighlighter
from rich.theme import Theme

logger = parent_logger.getChild("cmds")


class Ctx:
    _db: ReleasesDB | None
    github_token: str | None
    ceph_git_path: Path | None

    def __init__(self) -> None:
        self._db = None  # ReleasesDB(Path.cwd().joinpath(".releases"))
        self.github_token = None
        self.ceph_git_path = None

    def init(self, path: Path, secrets: SecretsVaultMgr) -> None:
        self._db = ReleasesDB(path, secrets)

    @property
    def db(self) -> ReleasesDB:
        if not self._db:
            raise CRTError(msg="database not initialized")
        return self._db

    @property
    def db_path(self) -> Path:
        return self.db.base_path


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)


class _CRTHighlighter(RegexHighlighter):
    base_style: str = "crt."
    highlights: list[str] = [  # noqa: RUF012
        r"(?P<uuid>\w{8}-\w{4}-\w{4}-\w{4}-\w{12})",
        r"(?P<sha>[a-f0-9]{40})",
        r"(?P<sha>[a-f0-9]{64})",
    ]


_theme = Theme(
    {
        "crt.uuid": "gold1",
        "crt.sha": "purple",
    }
)
console = Console(highlighter=_CRTHighlighter(), theme=_theme)


def perror(s: str) -> None:
    console.print(
        f"[bold][red]error:[/red] {s}[/bold]",
    )


def pinfo(s: str) -> None:
    console.print(s, style="cyan")


def psuccess(s: str) -> None:
    console.print(s, style="bold green")


def pwarn(s: str) -> None:
    console.print(f"[bold yellow]warning:[/bold yellow] {s}")


def rprint(s: str) -> None:
    console.print(s)


def set_debug_logging() -> None:
    parent_logger.setLevel(logging.DEBUG)


_R = TypeVar("_R")
_T = TypeVar("_T")
_P = ParamSpec("_P")


def pass_db(f: Callable[Concatenate[ReleasesDB, _P], _R]) -> Callable[_P, _R]:
    """Pass the release database instance to the function."""

    def inner(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            perror(f"missing context for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)
        return f(ctx.db, *args, **kwargs)

    return update_wrapper(inner, f)


def pass_s3db(f: Callable[Concatenate[S3DB, _P], _R]) -> Callable[_P, _R]:
    """Pass the releases S3 database instance to the function."""

    def inner(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            perror(f"missing context for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)
        try:
            return f(ctx.db.s3db, *args, **kwargs)
        except DBError:
            perror(f"s3 db not init for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)

    return update_wrapper(inner, f)
