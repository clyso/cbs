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
from crtlib.db import ReleasesDB
from crtlib.errors.manifest import (
    MalformedManifestError,
    ManifestError,
    NoSuchManifestError,
)
from crtlib.errors.patchset import PatchSetError
from crtlib.manifest import manifest_execute
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patch import Patch
from crtlib.models.patchset import GitHubPullRequest
from rich.console import Group, RenderableType
from rich.padding import Padding
from rich.panel import Panel
from rich.table import Table
from rich.tree import Tree

from . import Ctx, console, pass_ctx, perror, pinfo
from . import logger as parent_logger

logger = parent_logger.getChild("manifest")


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


def _get_manifests_from_uuids(
    db: ReleasesDB, uuid_lst: list[uuid.UUID]
) -> list[ReleaseManifest]:
    lst: list[ReleaseManifest] = []
    for entry in uuid_lst:
        try:
            manifest = db.load_manifest(entry)
        except ManifestError as e:
            logger.error(f"unable to load manifest: {e}")
            continue
        lst.append(manifest)

    return sorted(lst, key=lambda e: e.creation_date)


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

    manifest_path = ctx.db.manifests_path.joinpath(f"{manifest.release_uuid}.json")
    if manifest_path.exists():
        perror(
            "conflicting manifest UUID, " + f"'{manifest.release_uuid}' already exists",
        )
        sys.exit(errno.EEXIST)

    try:
        ctx.db.store_manifest(manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
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
    lst = _get_manifests_from_uuids(ctx.db, ctx.db.list_manifests_uuids())
    for manifest in lst:
        table = _gen_rich_manifest_table(manifest)
        table.title = f"Manifest {manifest.release_uuid}"
        console.print(Padding(table, (0, 0, 1, 0)))


@cmd_manifest.command("info", help="Show information about release manifests.")
@click.option(
    "-m",
    "--manifest-uuid",
    required=False,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID for which information will be shown.",
)
@pass_ctx
def cmd_manifest_info(ctx: Ctx, manifest_uuid: uuid.UUID | None) -> None:
    db = ctx.db

    manifest_uuids_lst = [manifest_uuid] if manifest_uuid else db.list_manifests_uuids()
    lst = _get_manifests_from_uuids(db, manifest_uuids_lst)

    for manifest in lst:
        table = _gen_rich_manifest_table(manifest)

        patchsets_lst: list[RenderableType] = []

        for patchset_uuid in manifest.patchsets:
            try:
                patchset = db.load_patchset(patchset_uuid)
            except (PatchSetError, Exception) as e:
                perror(
                    f"unable to load patch set uuid '{patchset_uuid}': {e}",
                )
                sys.exit(errno.ENOTRECOVERABLE)

            patchset_tree = Tree(f"\u276f [blue]{patchset.title}")
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
                patch_tree = Tree(f"\u2022 [green]{patch.title}")

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
            patchsets_lst.append(patchset_tree)

        patchsets_group = (
            Group(*patchsets_lst) if patchsets_lst else Group("[bold red]None")
        )

        panel = Panel(
            Group(
                table, "", "[red]patch sets:", Padding(patchsets_group, (0, 0, 0, 2))
            ),
            box=rich.box.SQUARE,
            title=f"Manifest {manifest.release_uuid}",
        )
        console.print(panel)


@cmd_manifest.command("exec", help="Apply a release manifest.")
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
@click.option(
    "--push", is_flag=True, required=False, default=False, help="Push to repository."
)
@pass_ctx
def cmd_manifest_exec(
    ctx: Ctx,
    manifest_uuid: uuid.UUID,
    ceph_git_path: Path,
    repo: str,
    push: bool,
) -> None:
    logger.debug(f"apply manifest uuid '{manifest_uuid}' to repo '{ceph_git_path}'")
    logger.debug(f"push to repository: {push}, repository: {repo}")

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

    pinfo("apply manifest")
    try:
        res = manifest_execute(ctx.db, manifest, ceph_git_path, ctx.github_token, push)
    except ApplyConflictError as e:
        perror(f"{len(e.conflict_files)} file conflicts found applying manifest")
        pinfo(f"[bold]on sha '{e.sha}':[/bold]")
        for file in e.conflict_files:
            pinfo(f"\u203a {file}")

        sys.exit(errno.EAGAIN)

    except ApplyError as e:
        perror(f"unable to apply manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

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

    push_summary_table = Table(show_header=False, show_lines=False, box=None)
    push_summary_table.add_column(justify="right", style="cyan", no_wrap=True)
    push_summary_table.add_column(justify="left", style="magenta", no_wrap=True)
    push_summary_table.add_row("remote", manifest.dst_repo)
    push_summary_table.add_row("remote updated", str(res.remote_updated))
    if res.heads_rejected:
        push_summary_table.add_row("heads rejected", ", ".join(res.heads_rejected))
    if res.heads_updated:
        push_summary_table.add_row("heads updated", ", ".join(res.heads_updated))

    applied_str = "applied" if res.applied else "[red]not[/red] applied"
    apply_summary_str = (
        f"[bold]Manifest {applied_str} to branch "
        + f"'[gold1]{res.target_branch}[/gold1]'[/bold]"
    )
    apply_summary_group = Group(apply_summary_str, "", apply_summary_table)

    push_str = "pushed" if res.pushed_to_remote else "[red]not[/red] pushed"
    push_summary_str = (
        f"[bold]Branch '[gold1]{res.target_branch}[/gold1]' {push_str} to "
        + f"'[gold1]{manifest.dst_repo}[/gold1]'[/bold]"
    )
    push_summary_group_lst: list[RenderableType] = [push_summary_str]
    if res.pushed_to_remote:
        push_summary_group_lst.extend(["", push_summary_table])
    push_summary_group = Group(*push_summary_group_lst)

    panel = Panel(
        Group(apply_summary_group, "", push_summary_group),
        box=rich.box.SQUARE,
        title=f"Manifest {manifest.release_uuid}",
    )

    console.print(panel)
