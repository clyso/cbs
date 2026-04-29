# CBS Release Tool - store commands
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
import sys
from pathlib import Path

import click
import git

from . import console, perror, psuccess, with_patches_repo_path
from . import logger as parent_logger

logger = parent_logger.getChild("store")


@click.group("store", help="CRT store operations.")
def cmd_store() -> None:
    pass


@cmd_store.command("sync", help="Merge main into the current release branch.")
@with_patches_repo_path
def cmd_store_sync(patches_repo_path: Path) -> None:
    try:
        repo = git.Repo(patches_repo_path)
    except Exception as e:
        perror(f"unable to open store repository: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        current_branch = repo.active_branch.name
    except Exception as e:
        perror(f"unable to determine active branch: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if current_branch == "main":
        perror("already on main, nothing to sync")
        sys.exit(errno.EINVAL)

    if not current_branch.startswith("release/"):
        perror(f"current branch '{current_branch}' is not a release branch")
        sys.exit(errno.EINVAL)

    try:
        repo.git.merge(  # pyright: ignore[reportAny]
            "main", m=f"Merge main into {current_branch}"
        )
    except git.GitCommandError as e:
        perror(f"merge conflict or error merging main: {e}")
        console.print("[yellow]resolve conflicts manually, then commit[/yellow]")
        sys.exit(errno.EAGAIN)

    psuccess(f"merged main into '{current_branch}'")
