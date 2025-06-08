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

import sys
from pathlib import Path

import click
from crtlib.db import ReleasesDB
from crtlib.logger import logger as parent_logger
from rich import print as rprint

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


def perror(s: str) -> None:
    rprint(f"[bold][red]error:[/red] {s}[/bold]", file=sys.stderr)


def pinfo(s: str) -> None:
    rprint(f"[cyan]{s}[/cyan]")


def psuccess(s: str) -> None:
    rprint(f"[bold green]{s}[/bold green]")
