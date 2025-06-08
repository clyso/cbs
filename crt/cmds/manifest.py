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
from crtlib.db import ReleasesDB
from crtlib.logger import logger
from crtlib.manifest import (
    MalformedManifestError,
    ManifestError,
    NoSuchManifestError,
    ReleaseManifest,
)
from crtlib.patchset import GitHubPullRequest, PatchSetError
from rich.console import Console, Group, RenderableType
from rich.padding import Padding
from rich.panel import Panel
from rich.table import Table
from rich.tree import Tree

from cmds import Ctx, pass_ctx

console = Console()


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
@pass_ctx
def cmd_manifest_create(ctx: Ctx, name: str, base_release: str, base_ref: str) -> None:
    m = re.match(r"(?:(.+)@)?([\w\d_.-]+)", base_ref)
    if not m:
        click.echo("error: malformed BASE_REF")
        sys.exit(errno.EINVAL)

    base_repo_str = cast(str | None, m.group(1))
    base_ref_str = cast(str, m.group(2))
    if not base_repo_str:
        base_repo_str = "clyso/ceph"

    m = re.match(r"([\w\d_.-]+)/([\w\d_.-]+)", base_repo_str)
    if not m:
        click.echo("error: malformed REPO")
        sys.exit(errno.EINVAL)

    base_repo_org = cast(str, m.group(1))
    base_repo = cast(str, m.group(2))

    manifest = ReleaseManifest(
        name=name,
        base_release_name=base_release,
        base_ref_org=base_repo_org,
        base_ref_repo=base_repo,
        base_ref=base_ref_str,
    )

    manifest_path = ctx.db.manifests_path.joinpath(f"{manifest.release_uuid}.json")
    if manifest_path.exists():
        click.echo(
            "error: conflicting manifest UUID, "
            + f"'{manifest.release_uuid}' already exists",
            err=True,
        )
        sys.exit(errno.EEXIST)

    try:
        ctx.db.store_manifest(manifest)
    except Exception as e:
        click.echo(f"error: unable to write manifest to disk: {e}")
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
                click.echo(
                    f"error: unable to load patch set uuid '{patchset_uuid}': {e}",
                    err=True,
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


@cmd_manifest.command("apply", help="Apply a release manifest.")
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
def cmd_manifest_apply(
    ctx: Ctx, manifest_uuid: uuid.UUID, _ceph_git_path: Path
) -> None:
    try:
        _manifest = ctx.db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        click.echo(f"error: unable to find manifest '{manifest_uuid}'", err=True)
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        click.echo(f"error: malformed manifest '{manifest_uuid}'", err=True)
        sys.exit(errno.EINVAL)
    except Exception as e:
        click.echo(f"error: unable to load manifest '{manifest_uuid}': {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)
