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

import logging
from pathlib import Path

import click
from crtlib.db.db import ReleasesDB
from crtlib.logger import logger as parent_logger
from rich.console import Console
from rich.highlighter import RegexHighlighter
from rich.theme import Theme

logger = parent_logger.getChild("cmds")


class Ctx:
    db: ReleasesDB
    github_token: str | None
    ceph_git_path: Path | None

    def __init__(self) -> None:
        self.db = ReleasesDB(Path.cwd().joinpath(".releases"))
        self.github_token = None
        self.ceph_git_path = None

    @property
    def db_path(self) -> Path:
        return self.db.db_path

    @db_path.setter
    def db_path(self, path: Path) -> None:
        self.db.db_path = path


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)


class _CRTHighlighter(RegexHighlighter):
    base_style: str = "crt."
    highlights: list[str] = [  # noqa: RUF012
        r"(?P<uuid>\w{8}-\w{4}-\w{4}-\w{4}-\w{12})",
        r"(?P<sha>[a-f0-9]{40})",
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
