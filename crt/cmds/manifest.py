# Ceph Release Tool - manifest commands
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
import re
import sys
import uuid
from pathlib import Path
from typing import cast

import click
import rich.box
from crtlib.apply import ApplyConflictError, ApplyError
from crtlib.db.db import ReleasesDB
from crtlib.errors.manifest import (
    MalformedManifestError,
    ManifestError,
    ManifestExistsError,
    NoSuchManifestError,
)
from crtlib.errors.patchset import PatchSetError
from crtlib.logger import logger_set_handler, logger_unset_handler
from crtlib.manifest import (
    ManifestExecuteResult,
    manifest_create,
    manifest_execute,
    manifest_publish_branch,
)
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patch import Patch
from crtlib.models.patchset import GitHubPullRequest
from rich.console import Group, RenderableType
from rich.logging import RichHandler
from rich.padding import Padding
from rich.panel import Panel
from rich.progress import Progress, SpinnerColumn, TextColumn, TimeElapsedColumn
from rich.rule import Rule
from rich.table import Table
from rich.tree import Tree

from cmds import stages

from . import Ctx, Symbols, console, pass_ctx, perror, pinfo, pwarn
from . import logger as parent_logger

logger = parent_logger.getChild("manifest")


class _ExitError(Exception):
    code: int

    def __init__(self, ec: int) -> None:
        super().__init__()
        self.code = ec


def _gen_rich_manifest_table(manifest: ReleaseManifest) -> Table:
    table = Table(
        show_header=False,
        show_lines=False,
        box=None,
    )
    table.add_column(justify="right", style="cyan", no_wrap=True)
    table.add_column(justify="left", style="magenta", no_wrap=True)

    for row in manifest.gen_header():
        table.add_row(*row)

    return table


@click.group("manifest", help="Manifest operations.")
def cmd_manifest() -> None:
    pass


@cmd_manifest.command("create", help="Create a new release manifest.")
@click.argument("name", type=str, required=True, metavar="NAME")
@click.argument("base_release", type=str, required=True, metavar="BASE_RELEASE")
@click.argument("base_ref", type=str, required=True, metavar="[REPO@]REF")
@click.option(
    "-r",
    "--dst-repo",
    type=str,
    required=False,
    metavar="ORG/REPO",
    default="clyso/ceph",
    help="Destination repository.",
)
@pass_ctx
def cmd_manifest_create(
    ctx: Ctx, name: str, base_release: str, base_ref: str, dst_repo: str
) -> None:
    m = re.match(r"(?:(.+)@)?([\w\d_.-]+)", base_ref)
    if not m:
        perror("malformed BASE_REF")
        sys.exit(errno.EINVAL)

    base_repo_str = cast(str | None, m.group(1))
    base_ref_str = cast(str, m.group(2))
    if not base_repo_str:
        base_repo_str = "clyso/ceph"

    repo_re = re.compile(r"([\w\d_.-]+)/([\w\d_.-]+)")

    m = re.match(repo_re, base_repo_str)
    if not m:
        perror("malformed base reference REPO")
        sys.exit(errno.EINVAL)

    base_repo_org = cast(str, m.group(1))
    base_repo = cast(str, m.group(2))

    if not re.match(repo_re, dst_repo):
        perror("malformed destination repository, use 'ORG/REPO'")
        sys.exit(errno.EINVAL)

    manifest = ReleaseManifest(
        name=name,
        base_release_name=base_release,
        base_ref_org=base_repo_org,
        base_ref_repo=base_repo,
        base_ref=base_ref_str,
        dst_repo=dst_repo,
    )

    try:
        manifest_create(ctx.db, manifest)
    except ManifestError as e:
        perror(f"unable to create manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    table = _gen_rich_manifest_table(manifest)
    panel = Panel(
        Group(
            table,
            Padding(
                "[bold]You can now modify this release using its UUID", (1, 0, 1, 2)
            ),
        ),
        box=rich.box.SQUARE,
        title="Manifest Created",
    )
    console.print(panel)


@cmd_manifest.command("list", help="List existing release manifest.")
@pass_ctx
def cmd_manifest_list(ctx: Ctx) -> None:
    lst = ctx.db.list_manifests(from_remote=True)
    for entry in lst:
        classifiers: list[str] = []
        if entry.local:
            classifiers.append("[bold green]staged[/bold green]")
        if entry.from_s3:
            classifiers.append("[bold gold1]remote[/bold gold1]")
        if entry.modified:
            classifiers.append("[bold red]modified[/bold red]")

        classifiers_str_lst = ", ".join(classifiers)
        classifiers_str = f" ({classifiers_str_lst})" if classifiers_str_lst else ""

        table = _gen_rich_manifest_table(entry.manifest)
        console.print(
            Panel(
                table,
                title=f"Manifest {entry.manifest.release_uuid}{classifiers_str}",
                box=rich.box.SQUARE,
            )
        )


@cmd_manifest.command("info", help="Show information about release manifests.")
@click.option(
    "-m",
    "--manifest-uuid",
    required=False,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID for which information will be shown.",
)
@click.option(
    "-s",
    "--stages",
    required=False,
    is_flag=True,
    default=False,
    help="Show stages information.",
)
@pass_ctx
def cmd_manifest_info(ctx: Ctx, manifest_uuid: uuid.UUID | None, stages: bool) -> None:
    db = ctx.db

    lst = db.list_manifests(from_remote=True)

    def _patchset_entry(
        patchsets: list[uuid.UUID], uncommitted: bool | None = None
    ) -> list[RenderableType]:
        patchsets_tree_lst: list[RenderableType] = []
        for patchset_uuid in patchsets:
            try:
                patchset, is_remote = db.load_patchset(patchset_uuid)
            except (PatchSetError, Exception) as e:
                perror(
                    f"unable to load patch set uuid '{patchset_uuid}': {e}",
                )
                pwarn("maybe a remote patch set that needs to be sync'ed?")
                patchsets_tree_lst.append(
                    f"[bold][red]missing patchset UUID[/red] '{patchset_uuid}'"
                )
                continue

            classifiers_lst: list[str] = []
            if uncommitted:
                classifiers_lst.append("[bold magenta]uncommitted[/bold magenta]")
            if is_remote:
                classifiers_lst.append("[bold gold1]remote[/bold gold1]")
            classifiers_str = ", ".join(classifiers_lst)
            classifiers_str = f" ({classifiers_str})" if classifiers_str else ""
            patchset_tree = Tree(
                f"{Symbols.RIGHT_ARROW} [blue]{patchset.title}{classifiers_str}"
            )
            patchset_table = Table(show_header=False, show_lines=False, box=None)
            patchset_table.add_column(justify="right", style="cyan", no_wrap=True)
            patchset_table.add_column(justify="left", style="magenta", no_wrap=True)

            patchset_table.add_row(
                "author", f"{patchset.author.user} <{patchset.author.email}>"
            )
            patchset_table.add_row("created", str(patchset.creation_date))
            patchset_table.add_row("related", "\n".join(patchset.related_to))

            if isinstance(patchset, GitHubPullRequest):
                patchset_table.add_row("repo", patchset.repo_url)
                patchset_table.add_row("pr id", str(patchset.pull_request_id))
                patchset_table.add_row("target", patchset.target_branch)
                patchset_table.add_row("merged", str(patchset.merge_date))

            patches_table = Table(show_header=False, show_lines=False, box=None)
            patches_table.add_column(justify="left", no_wrap=True)

            for patch in patchset.patches:
                patch_tree = Tree(f"{Symbols.BULLET} [green]{patch.title}")

                patch_table = Table(show_header=False, show_lines=False, box=None)
                patch_table.add_column(justify="right", style="cyan", no_wrap=True)
                patch_table.add_column(justify="left", style="magenta", no_wrap=True)

                patch_table.add_row(
                    "author", f"{patch.author.user} <{patch.author.email}>"
                )
                patch_table.add_row("date", str(patch.author_date))
                if patch.related_to:
                    patch_table.add_row("related", "\n".join(patch.related_to))
                if patch.cherry_picked_from:
                    patch_table.add_row(
                        "cherry-picked from", "\n".join(patch.cherry_picked_from)
                    )

                _ = patch_tree.add(patch_table)
                patches_table.add_row(Padding(patch_tree, (0, 0, 1, 0)))

            patchset_table.add_row("patches", Group("", patches_table))

            _ = patchset_tree.add(patchset_table)
            patchsets_tree_lst.append(patchset_tree)

        return patchsets_tree_lst

    for entry in lst:
        manifest = entry.manifest
        if manifest_uuid and manifest.release_uuid != manifest_uuid:
            continue

        table = _gen_rich_manifest_table(manifest)

        stages_renderables: list[RenderableType] = []
        stage_n = 1
        for stage in manifest.stages:
            stage_rdr_lst: list[RenderableType] = []

            if stages:
                stage_table = Table(show_header=False, show_lines=False, box=None)
                stage_table.add_column(justify="right", style="cyan", no_wrap=True)
                stage_table.add_column(justify="left", style="magenta", no_wrap=True)
                stage_table.add_row(
                    "author", f"{stage.author.user} <{stage.author.email}>"
                )
                stage_table.add_row("created", str(stage.creation_date))
                stage_table.add_row("committed", "Yes" if stage.committed else "No")
                if stage.committed:
                    stage_table.add_row("hash", stage.hash)
                stage_table.add_row("patch sets", str(len(stage.patchsets)))
                stage_rdr_lst.append(Padding(stage_table, (0, 0, 1, 0)))

            stage_rdr_lst.extend(_patchset_entry(stage.patchsets, not stage.committed))

            committed_str = " (uncommitted)" if not stage.committed else ""
            title_str = (
                f"[bold]{Symbols.DOWN_ARROW} Stage #{stage_n}{committed_str}[/bold]"
            )
            stages_renderables.append(
                Group(
                    Padding(
                        Rule(
                            title_str,
                            align="left",
                        ),
                        (0, 0, 1, 0),
                    ),
                    *stage_rdr_lst,
                ),
            )
            stage_n += 1

        stages_group = (
            Group(*stages_renderables)
            if stages_renderables
            else Group("[bold red]None")
        )

        classifiers: list[str] = []
        if entry.local:
            classifiers.append("[bold green]staged[/bold green]")
        if entry.from_s3:
            classifiers.append("[bold gold1]remote[/bold gold1]")
        if entry.modified:
            classifiers.append("[bold red]modified[/bold red]")

        classifiers_str_lst = ", ".join(classifiers)
        classifiers_str = f" ({classifiers_str_lst})" if classifiers_str_lst else ""

        panel = Panel(
            Group(
                table,
                Padding("[red]patch sets:", (1, 0, 1, 0)),
                Padding(stages_group, (0, 0, 0, 2)),
            ),
            box=rich.box.SQUARE,
            title=f"Manifest {manifest.release_uuid}{classifiers_str}",
        )
        console.print(panel)


def _manifest_execute(
    db: ReleasesDB,
    manifest: ReleaseManifest,
    *,
    token: str,
    repo_path: Path,
    no_cleanup: bool = True,
    progress: Progress | None = None,
) -> tuple[ManifestExecuteResult, RenderableType]:
    """
    Execute a manifest and return a renderable for the console.

    This function is shared between 'validate' and 'publish'.
    """
    has_progress = progress is not None
    if not has_progress:
        progress = Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            TimeElapsedColumn(),
            console=console,
        )
        progress.start()

    progress_task = progress.add_task("executing manifest")
    progress.start_task(progress_task)

    try:
        res = manifest_execute(db, manifest, repo_path, token, no_cleanup=no_cleanup)
    except ApplyConflictError as e:
        perror(f"{len(e.conflict_files)} file conflicts found applying manifest")
        pinfo(f"[bold]on sha '{e.sha}':[/bold]")
        for file in e.conflict_files:
            pinfo(f"{Symbols.SMALL_RIGHT_ARROW} {file}")
        sys.exit(errno.EAGAIN)

    except ApplyError as e:
        perror(f"unable to apply manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    progress.stop_task(progress_task)
    if not has_progress:
        progress.stop()

    def _gen_patches_table(patches: list[Patch]) -> Table:
        table = Table(show_header=False, show_lines=False, box=None)
        table.add_column(justify="left", style="blue")

        for patch in patches:
            table.add_row(patch.title)

        if not patches:
            table.add_row("[bold]None[/bold]")
        else:
            table.add_row(f"[bold]total:[/bold] {len(patches)}")
        return table

    apply_summary_table = Table(show_header=False, show_lines=False, box=None)
    apply_summary_table.add_column(justify="right", style="cyan", no_wrap=True)
    apply_summary_table.add_column(justify="left", style="magenta", no_wrap=True)
    patches_added_renderable = _gen_patches_table(res.added)
    patches_skipped_renderable = _gen_patches_table(res.skipped)
    apply_summary_table.add_row("patches added", patches_added_renderable)
    apply_summary_table.add_row("patches skipped", patches_skipped_renderable)

    applied_str = "applied" if res.applied else "[red]not[/red] applied"
    apply_summary_str = (
        f"[bold]Manifest {applied_str} to branch "
        + f"'[gold1]{res.target_branch}[/gold1]'[/bold]"
    )
    apply_summary_group = Group(
        Padding(apply_summary_str, (0, 0, 1, 0)),
        apply_summary_table,
    )

    return (res, apply_summary_group)


def _manifest_publish(
    db: ReleasesDB,
    manifest: ReleaseManifest,
    repo_path: Path,
    our_branch: str,
    progress: Progress,
) -> RenderableType:
    publish_task = progress.add_task("publishing")
    publish_manifest_task = progress.add_task("publish manifest")
    publish_branch_task = progress.add_task("publish branch")

    progress.start_task(publish_task)

    progress.start_task(publish_manifest_task)
    try:
        db.publish_manifest(manifest.release_uuid)
    except ManifestExistsError:
        perror(f"manifest '{manifest.release_uuid}' already published")
        pwarn("maybe run [bold bright_magenta]'db sync'[/bold bright_magenta] first?")
        raise _ExitError(errno.EEXIST) from None

    progress.stop_task(publish_manifest_task)

    progress.start_task(publish_branch_task)
    try:
        res = manifest_publish_branch(manifest, repo_path, our_branch)
    except ManifestError as e:
        perror(f"unable to publish manifest '{manifest.release_uuid}': {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from None

    progress.stop_task(publish_branch_task)
    progress.stop_task(publish_task)

    push_summary_table = Table(show_header=False, show_lines=False, box=None)
    push_summary_table.add_column(justify="right", style="cyan", no_wrap=True)
    push_summary_table.add_column(justify="left", style="magenta", no_wrap=True)
    push_summary_table.add_row("remote", manifest.dst_repo)
    push_summary_table.add_row("remote updated", str(res.remote_updated))
    if res.heads_rejected:
        push_summary_table.add_row("heads rejected", ", ".join(res.heads_rejected))
    if res.heads_updated:
        push_summary_table.add_row("heads updated", ", ".join(res.heads_updated))

    push_summary_str = (
        f"[bold]Branch '[gold1]{our_branch}[/gold1]' published to "
        + f"'[gold1]{manifest.dst_repo}[/gold1]'[/bold]"
    )

    return Group(
        Padding(push_summary_str, (0, 0, 1, 0)),
        push_summary_table,
    )


@cmd_manifest.command("validate", help="Validate locally a release manifest.")
@click.argument("manifest_uuid", type=uuid.UUID, required=True, metavar="UUID")
@click.option(
    "-c",
    "--ceph-git-path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository.",
)
@pass_ctx
def cmd_manifest_validate(
    ctx: Ctx,
    manifest_uuid: uuid.UUID,
    ceph_git_path: Path,
) -> None:
    logger.debug(f"apply manifest uuid '{manifest_uuid}' to repo '{ceph_git_path}'")

    if not ctx.github_token:
        perror("missing github token")
        sys.exit(errno.EINVAL)

    try:
        manifest = ctx.db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_uuid}'")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load manifest '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    (_, apply_summary) = _manifest_execute(
        ctx.db,
        manifest,
        token=ctx.github_token,
        repo_path=ceph_git_path,
        no_cleanup=False,
    )

    panel = Panel(
        apply_summary,
        box=rich.box.SQUARE,
        title=f"Manifest {manifest.release_uuid}",
    )
    console.print(panel)


@cmd_manifest.command("publish")
@click.argument("manifest_uuid", type=uuid.UUID, required=True, metavar="UUID")
@click.option(
    "-c",
    "--ceph-git-path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository.",
)
@click.option(
    "-r",
    "--repo",
    type=str,
    required=False,
    default="clyso/ceph",
    metavar="ORG/REPO",
    help="Repository to push to.",
)
@pass_ctx
def cmd_manifest_publish(
    ctx: Ctx,
    manifest_uuid: uuid.UUID,
    ceph_git_path: Path,
    repo: str,
) -> None:
    """
    Publish a manifest.

    Will upload the manifest to S3, and push a branch to the specified repository.
    """
    logger.debug(f"commit manifest uuid '{manifest_uuid}'")

    if not ctx.github_token:
        perror("missing github token")
        sys.exit(errno.EINVAL)

    if not re.match(r"^[\w_.-]+/[\w_.-]+", repo):
        perror("malformed repository, use ORG/REPO")
        sys.exit(errno.EINVAL)

    try:
        manifest = ctx.db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_uuid}'")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load manifest '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not manifest.committed:
        perror(f"manifest '{manifest_uuid}' not committed")
        pwarn("run '[bold bright_magenta]manifest stage commit[/bold bright_magenta]'")
        sys.exit(errno.EBUSY)

    rich_handler = RichHandler(console=console)
    logger_set_handler(rich_handler)

    progress = Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        TimeElapsedColumn(),
        console=console,
    )
    progress.start()

    execute_res, execute_summary = _manifest_execute(
        ctx.db,
        manifest,
        token=ctx.github_token,
        repo_path=ceph_git_path,
        no_cleanup=True,
        progress=progress,
    )

    try:
        publish_summary = _manifest_publish(
            ctx.db,
            manifest,
            ceph_git_path,
            execute_res.target_branch,
            progress,
        )
    except _ExitError as e:
        progress.stop()
        sys.exit(e.code)

    progress.stop()

    logger_unset_handler(rich_handler)

    panel = Panel(
        Group(
            Padding(execute_summary, (0, 0, 1, 0)),
            Padding(publish_summary, (0, 0, 1, 0)),
        ),
        title=f"Manifest {manifest_uuid}",
        box=rich.box.SQUARE,
    )
    console.print(panel)
    pass


cmd_manifest.add_command(stages.cmd_manifest_stage)
