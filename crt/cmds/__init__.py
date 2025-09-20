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

import enum
import errno
import logging
import sys
from collections.abc import Callable
from functools import update_wrapper
from pathlib import Path
from typing import Concatenate, ParamSpec, TypeVar

import click
from crtlib.logger import logger as parent_logger
from rich.console import Console
from rich.highlighter import RegexHighlighter
from rich.theme import Theme

logger = parent_logger.getChild("cmds")


class Ctx:
    github_token: str | None
    patches_repo_path: Path | None

    def __init__(self) -> None:
        self.github_token = None
        self.patches_repo_path = None


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)


_R = TypeVar("_R")
_T = TypeVar("_T")
_P = ParamSpec("_P")


def with_patches_repo_path(f: Callable[Concatenate[Path, _P], _R]) -> Callable[_P, _R]:
    """Pass the CES patches repo path from the context to the function."""

    def inner(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            perror(f"missing context for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)
        if not ctx.patches_repo_path:
            perror("CES patches repo path not provided")
            sys.exit(errno.EINVAL)
        return f(ctx.patches_repo_path, *args, **kwargs)

    return update_wrapper(inner, f)


def with_gh_token(f: Callable[Concatenate[str, _P], _R]) -> Callable[_P, _R]:
    """Pass the GitHub token from the context to the function."""

    def inner(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            perror(f"missing context for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)
        if not ctx.github_token:
            perror("GitHub token not provided")
            sys.exit(errno.EINVAL)
        return f(ctx.github_token, *args, **kwargs)

    return update_wrapper(inner, f)


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


def set_verbose_logging() -> None:
    parent_logger.setLevel(logging.INFO)


class Symbols(enum.StrEnum):
    RIGHT_ARROW = "\u276f"  # '>'
    BULLET = "\u2022"
    SMALL_RIGHT_ARROW = "\u203a"
    DOWN_ARROW = "\u2304"
    CHECK_MARK = "\u2713"
    CROSS_MARK = "\u2717"
