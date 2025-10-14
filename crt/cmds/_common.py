# CBS Release Tool - common cli functions
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

from pathlib import Path

from crtlib.models.discriminator import ManifestPatchEntryWrapper
from crtlib.models.manifest import ManifestStage
from crtlib.models.patch import Patch
from crtlib.models.patchset import CustomPatchSet, GitHubPullRequest
from rich.console import Console, Group, RenderableType
from rich.padding import Padding
from rich.progress import (
    Progress,
    SpinnerColumn,
    TaskID,
    TextColumn,
    TimeElapsedColumn,
)
from rich.rule import Rule
from rich.table import Table
from rich.text import Text
from rich.tree import Tree

from cmds import Symbols, perror


def get_stage_summary_rdr(stage: ManifestStage) -> RenderableType:
    table = Table(
        show_header=False,
        show_lines=False,
        box=None,
    )
    table.add_column(justify="right", style="cyan", no_wrap=True)
    table.add_column(justify="left", style="magenta", no_wrap=True)

    table.add_row("uuid", str(stage.stage_uuid))
    table.add_row("author", f"{stage.author.user} <{stage.author.email}>")
    table.add_row("created", str(stage.creation_date))
    if stage.desc:
        table.add_row("description", stage.desc)
    table.add_row(
        "tags", (" ".join(f"{t}={n}" for t, n in stage.tags) if stage.tags else "None")
    )
    table.add_row("patch sets", str(len(stage.patches)))
    table.add_row(
        "published", "[green]yes[/green]" if stage.is_published else "[red]no[/red]"
    )

    return table


def _get_stage_patchset(
    patches_repo_path: Path,
    patches: list[ManifestPatchEntryWrapper],
    extended_info: bool = False,
) -> list[RenderableType]:
    def _do_patches_tree(patches: list[Patch]) -> list[RenderableType]:
        patches_tree_lst: list[RenderableType] = []

        has_previous = False
        for patch in patches:
            patch_tree = Tree(f"{Symbols.BULLET} [green]{patch.title}")

            patch_table = Table(show_header=False, show_lines=False, box=None)
            patch_table.add_column(justify="right", style="cyan", no_wrap=True)
            patch_table.add_column(justify="left", style="magenta", no_wrap=True)

            patch_table.add_row("author", f"{patch.author.user} <{patch.author.email}>")
            patch_table.add_row("date", str(patch.author_date))
            if patch.related_to:
                patch_table.add_row("related", "\n".join(patch.related_to))
            if patch.cherry_picked_from:
                patch_table.add_row(
                    "cherry-picked from", "\n".join(patch.cherry_picked_from)
                )

            _ = patch_tree.add(patch_table)
            patches_tree_lst.append(
                Padding(patch_tree, ((1 if has_previous else 0), 0, 0, 0))
            )
            has_previous = True

        return patches_tree_lst

    patches_tree_lst: list[RenderableType] = []
    for patch in patches:
        contents = patch.contents
        patch_meta_path = (
            patches_repo_path.joinpath("ceph")
            .joinpath("patches")
            .joinpath("meta")
            .joinpath(f"{contents.entry_uuid}.json")
        )

        if not patch_meta_path.exists():
            perror(f"missing patch set uuid '{contents.entry_uuid}")
            patches_tree_lst.append(
                "[bold][red]missing patch UUID[/red] "
                + f"'{contents.entry_uuid}'[/bold]"
            )
            continue

        patch_title = (
            contents.title
            if isinstance(contents, GitHubPullRequest | CustomPatchSet)
            else contents.info.title
        )
        patch_author = (
            contents.author
            if isinstance(contents, GitHubPullRequest | CustomPatchSet)
            else contents.info.author
        )
        patch_date = (
            contents.creation_date
            if isinstance(contents, GitHubPullRequest | CustomPatchSet)
            else contents.info.date
        )
        patch_fixes = "\n".join(
            contents.related_to
            if isinstance(contents, GitHubPullRequest | CustomPatchSet)
            else contents.info.fixes
        )

        classifiers_lst: list[str] = []
        classifiers_str = ", ".join(classifiers_lst)
        classifiers_str = f" ({classifiers_str})" if classifiers_str else ""
        patchset_tree = Tree(
            f"{Symbols.RIGHT_ARROW} [blue]{patch_title}{classifiers_str}"
        )

        patchset_table = Table(show_header=False, show_lines=False, box=None)
        patchset_table.add_column(justify="right", style="cyan", no_wrap=True)
        patchset_table.add_column(justify="left", style="magenta", no_wrap=True)

        patchset_table.add_row("uuid", str(contents.entry_uuid))
        patchset_table.add_row("author", f"{patch_author.user} <{patch_author.email}>")
        patchset_table.add_row("created", str(patch_date))
        if patch_fixes:
            patchset_table.add_row("related", patch_fixes)

        if isinstance(contents, GitHubPullRequest):
            patchset_table.add_row("repo", contents.repo_url)
            patchset_table.add_row("pr id", str(contents.pull_request_id))
            patchset_table.add_row("target", contents.target_branch)
            patchset_table.add_row("merged", str(contents.merge_date))

        elif isinstance(contents, CustomPatchSet):
            patchset_table.add_row("release", contents.release_name or "n/a")

        if isinstance(contents, GitHubPullRequest | CustomPatchSet) and extended_info:
            patches_table = Table(show_header=False, show_lines=False, box=None)
            patches_table.add_column(justify="left", no_wrap=True)
            patches_tree_rdr_lst = _do_patches_tree(contents.patches)
            for rdr in patches_tree_rdr_lst:
                patches_table.add_row(rdr)
            patchset_table.add_row("patches", Padding(patches_table, (1, 0, 0, 0)))

        _ = patchset_tree.add(Padding(patchset_table, (0, 0, 1, 0)))
        patches_tree_lst.append(patchset_tree)

    return patches_tree_lst


def get_stage_rdr(
    patches_repo_path: Path, stage: ManifestStage, extended_info: bool = False
) -> RenderableType:
    return Group(
        Padding(
            Rule(
                f"[bold]{Symbols.DOWN_ARROW} Stage {stage.stage_uuid}[/bold]",
                align="left",
            ),
            (1, 0, 0, 0),
        ),
        Padding(get_stage_summary_rdr(stage), (1, 0, 1, 0)),
        Padding(
            Group(
                *_get_stage_patchset(
                    patches_repo_path, stage.patches, extended_info=extended_info
                )
            ),
            (0, 0, 0, 2),
        ),
    )


class CRTProgress(Progress):
    _tasks_stack: list[TaskID]

    def __init__(self, console: Console) -> None:
        super().__init__(
            SpinnerColumn(finished_text=f"[green]{Symbols.CHECK_MARK}[/green]"),
            TextColumn("[progress.description]{task.description}"),
            TimeElapsedColumn(),
            console=console,
        )
        self._tasks_stack = []

    def new_task(self, description: str) -> None:
        """Start a new task, pushing the current one (if any) onto a stack."""
        task = self.add_task(description=description, start=True)
        self._tasks_stack.append(task)

    def done_task(self) -> None:
        """Finish the current task and pop the previous one (if any) from the stack."""
        if not self._tasks_stack:
            return

        current_task = self._tasks_stack.pop()
        self.update(current_task, completed=100)
        self.stop_task(current_task)

    def error_task(self) -> None:
        """Mark the current task as failed."""
        if not self._tasks_stack:
            return

        spinner = self.columns[0]
        assert isinstance(spinner, SpinnerColumn)
        spinner.finished_text = Text(Symbols.CROSS_MARK, style="red")

        current_task = self._tasks_stack.pop()
        self.update(current_task, completed=100)
        self.stop_task(current_task)

    def stop_error(self) -> None:
        """Stop all tasks and mark the current one as failed."""
        while self._tasks_stack:
            self.error_task()
        self.stop()


class CRTExitError(Exception):
    code: int

    def __init__(self, ec: int) -> None:
        super().__init__()
        self.code = ec
