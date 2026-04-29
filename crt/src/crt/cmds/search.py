# CBS Release Tool - patch search commands
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
import rich.box
from rich.padding import Padding
from rich.table import Table

from crt.cmds import console, perror, pwarn, with_patches_repo_path
from crt.crtlib.search import PatchSearchResult, search_patches


@click.command("search", help="Search the patch library.")
@click.option(
    "--grep",
    "grep_pattern",
    type=str,
    required=False,
    metavar="PATTERN",
    help="Filter patches by title (regex).",
)
@click.option(
    "--source",
    type=str,
    required=False,
    metavar="ORG/REPO",
    help="Filter by source repository.",
)
@click.option(
    "--pr",
    type=str,
    required=False,
    metavar="ORG/REPO#ID",
    help="Find a specific GitHub pull request.",
)
@click.option(
    "--uuid",
    "patch_uuid",
    type=str,
    required=False,
    metavar="UUID",
    help="Find a patch by UUID (prefix match).",
)
@with_patches_repo_path
def cmd_patch_search(
    patches_repo_path: Path,
    grep_pattern: str | None,
    source: str | None,
    pr: str | None,
    patch_uuid: str | None,
) -> None:
    if not any([grep_pattern, source, pr, patch_uuid]):
        perror("at least one search filter is required")
        sys.exit(errno.EINVAL)

    try:
        results = search_patches(
            patches_repo_path,
            grep=grep_pattern,
            source=source,
            pr=pr,
            patch_uuid=patch_uuid,
        )
    except ValueError as e:
        perror(str(e))
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"search failed: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not results:
        pwarn("no patches found")
        return

    table = _build_results_table(results)
    console.print(Padding(table, (1, 0, 1, 0)))


def _build_results_table(results: list[PatchSearchResult]) -> Table:
    table = Table(
        show_header=True,
        show_lines=True,
        box=rich.box.HORIZONTALS,
    )
    table.add_column("UUID", style="gold1", no_wrap=True)
    table.add_column("Title", style="white")
    table.add_column("Source", style="cyan")
    table.add_column("PR", style="magenta")

    for r in results:
        pr_str = str(r.pr_id) if r.pr_id else ""
        table.add_row(str(r.entry_uuid), r.title, r.source, pr_str)

    return table
