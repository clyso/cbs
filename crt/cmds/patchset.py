# Ceph Release Tool - patchset commands
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

import datetime
import errno
import re
import sys
import uuid
from datetime import datetime as dt
from pathlib import Path
from typing import cast

import click
import pydantic
import rich.box
from crtlib.errors.patchset import (
    MalformedPatchSetError,
)
from crtlib.git_utils import (
    SHA,
    git_branch_delete,
    git_get_patch_sha_title,
    git_patches_in_interval,
    git_prepare_remote,
)
from crtlib.models.common import (
    AuthorData,
    ManifestPatchEntry,
    ManifestPatchSetEntryType,
)
from crtlib.models.discriminator import ManifestPatchEntryWrapper
from crtlib.models.patch import PatchMeta
from crtlib.models.patchset import (
    CustomPatchMeta,
    CustomPatchSet,
    GitHubPullRequest,
    PatchSetBase,
)
from crtlib.patchset import (
    fetch_custom_patchset_patches,
    get_patchset_meta_path,
    load_patchset,
    write_patchset,
)
from rich.console import Group, RenderableType
from rich.padding import Padding
from rich.table import Table

from cmds import Ctx, console, pass_ctx, perror, pinfo, psuccess, pwarn
from cmds import logger as parent_logger

logger = parent_logger.getChild("patchset")


def _is_valid_sha(sha: str) -> bool:
    return re.match(r"^[\da-f]{4}[\da-f]{0,36}$", sha) is not None


def _gen_rich_patch_meta_info_header(table: Table, patch_meta: PatchMeta) -> Table:
    table.add_row("sha", f"[orchid2]{patch_meta.sha}[/orchid2]")
    table.add_row("title", patch_meta.info.title)
    table.add_row(
        "author", f"{patch_meta.info.author.user} <{patch_meta.info.author.email}>"
    )
    table.add_row("date", f"{patch_meta.info.date}")
    table.add_row("src version", patch_meta.src_version or "n/a")
    table.add_row("fixes", "\n".join(patch_meta.info.fixes) or "n/a")
    return table


def _gen_rich_patchset_base_info_header(table: Table, patchset: PatchSetBase) -> Table:
    table.add_row("title", patchset.title)
    table.add_row("author", f"{patchset.author.user} <{patchset.author.email}>")
    table.add_row("created", f"{patchset.creation_date}")
    table.add_row("related to", "\n".join(patchset.related_to) or "n/a")

    if isinstance(patchset, GitHubPullRequest):
        table.add_row("repo", f"{patchset.org_name}/{patchset.repo_name}")
        table.add_row("pr id", str(patchset.pull_request_id))
        table.add_row("updated on", str(patchset.updated_date) or "n/a")
        table.add_row("merged", "Yes" if patchset.merged else "No")
        if patchset.merged:
            table.add_row("merged on", str(patchset.merge_date) or "n/a")
        table.add_row("target branch", patchset.target_branch)
        table.add_row("patches", str(len(patchset.patches)))

    elif isinstance(patchset, CustomPatchSet):
        desc = patchset.description_text
        table.add_row("description", desc or "n/a")
        table.add_row("release", patchset.release_name or "n/a")
        table.add_row(
            "patches", str(sum(len(meta.patches) for meta in patchset.patches_meta))
        )
        table.add_row(
            "published",
            "[green]Yes[/green]" if patchset.is_published else "[red]No[/red]",
        )

    return table


def _gen_rich_patchset_info_header(table: Table, patchset: ManifestPatchEntry) -> Table:
    if isinstance(patchset, PatchMeta):
        return _gen_rich_patch_meta_info_header(table, patchset)
    elif isinstance(patchset, PatchSetBase):
        return _gen_rich_patchset_base_info_header(table, patchset)
    else:
        perror(f"unknown patch set type: {type(patchset)}")
        sys.exit(errno.ENOTRECOVERABLE)


def _gen_rich_patchset_info(patchset: ManifestPatchEntry) -> RenderableType:
    header_table = Table(show_header=False, box=None, expand=False)
    header_table.add_column(justify="left", style="bold cyan", no_wrap=False)
    header_table.add_column(justify="left", style="orange3", no_wrap=False)

    header_table.add_row("uuid", f"[gold1]{patchset.entry_uuid}[/gold1]")
    header_table.add_row("type", patchset.entry_type.value)
    header_table = _gen_rich_patchset_info_header(header_table, patchset)

    patch_lst_table = Table(
        show_header=False,
        show_lines=False,
        box=rich.box.HORIZONTALS,
    )
    patch_lst_table.add_column(justify="left", style="bold gold1", no_wrap=True)
    patch_lst_table.add_column(justify="left", style="white", no_wrap=False)

    if isinstance(patchset, GitHubPullRequest):
        for patch in patchset.patches:
            patch_lst_table.add_row(patch.sha, patch.title)
    elif isinstance(patchset, CustomPatchSet):
        for entry in patchset.patches_meta:
            for sha, title in entry.patches:
                patch_lst_table.add_row(sha, title)

    return Group(header_table, patch_lst_table)


def _gen_rich_patchset_list() -> Table:
    table = Table(
        show_header=False,
        show_lines=True,
        box=rich.box.HORIZONTALS,
    )
    # uuids
    table.add_column(justify="left", style="bold gold1", no_wrap=True)
    # type
    table.add_column(justify="left", style="bold cyan", no_wrap=True)
    # freeform entry
    table.add_column(justify="left", style="white", no_wrap=False)
    return table


def _gen_rich_patch_meta(patch_meta: PatchMeta) -> RenderableType:
    version = patch_meta.src_version if patch_meta.src_version else "n/a"

    # mimic what we would do with 'Columns', except that those will always take all
    # the available width within the console. And we want it to just fit to contents.
    t1 = Table(show_header=False, box=None, expand=False, padding=(0, 2, 0, 0))
    t1.add_column(justify="left")
    t1.add_column(justify="left")
    t1.add_row(
        f"[orchid2]{patch_meta.sha}[/orchid2]",
        f"[italic]version:[/italic] [orange3]{version}[/orange3]",
    )

    t2 = Table(show_header=False, box=None, expand=False, padding=(0, 2, 0, 0))
    t2.add_column(justify="left")
    t2.add_column(justify="left")
    t2.add_row(
        f"[cyan]{patch_meta.info.date}[/cyan]",
        f"[cyan]{patch_meta.info.author.user} "
        + f"<{patch_meta.info.author.email}>[/cyan]",
    )

    entries: list[RenderableType] = [
        f"[bold magenta]{patch_meta.info.title}[/bold magenta]",
        t1,
        t2,
    ]
    group = Group(*entries)
    return group


def _gen_rich_patchset_gh(patchset: GitHubPullRequest) -> RenderableType:
    is_merged = "[green]merged[/green]" if patchset.merged else "[red]not merged[/red]"
    repo_pr_id = f"{patchset.org_name}/{patchset.repo_name} #{patchset.pull_request_id}"
    t1 = Table(show_header=False, box=None, expand=False, padding=(0, 2, 0, 0))
    t1.add_row(
        f"[orchid2]{repo_pr_id}[/orchid2] ({is_merged})",
        f"[italic]version:[/italic] [orange3]{patchset.target_branch}[/orange3]",
    )

    updated = patchset.updated_date if patchset.updated_date else "n/a"
    merged = patchset.merge_date if patchset.merge_date else "n/a"
    t2 = Table(show_header=False, box=None, expand=False, padding=(0, 2, 0, 0))
    t2.add_row(
        f"[italic]updated:[/italic] [cyan]{updated}[/cyan]",
        f"[italic]merged:[/italic] [cyan]{merged}[/cyan]",
    )

    return Group(t1, t2)


def _gen_rich_patchset_custom(patchset: CustomPatchSet) -> RenderableType:
    release = patchset.release_name if patchset.release_name else "n/a"
    is_published = "[green]Yes[/green]" if patchset.is_published else "[red]No[/red]"
    t1 = Table(show_header=False, box=None, expand=False, padding=(0, 2, 0, 0))
    n_patches = sum(len(meta.patches) for meta in patchset.patches_meta)
    t1.add_row(
        f"[italic]release:[/italic] [orange3]{release}[/orange3]",
        f"[italic]published:[/italic] {is_published}",
        f"[italic]patches:[/italic] [orange3]{n_patches}[/orange3]",
    )
    return t1


def _gen_rich_patchset_base(patchset: PatchSetBase) -> RenderableType:
    # mimic what we would do with 'Columns', except that those will always take all
    # the available width within the console. And we want it to just fit to contents.

    t = Table(show_header=False, box=None, expand=False, padding=(0, 2, 0, 0))
    t.add_row(
        f"[cyan]{patchset.creation_date}[/cyan]",
        f"[cyan]{patchset.author.user} " + f"<{patchset.author.email}>[/cyan]",
    )

    rdr: RenderableType
    if isinstance(patchset, GitHubPullRequest):
        rdr = _gen_rich_patchset_gh(patchset)
    elif isinstance(patchset, CustomPatchSet):
        rdr = _gen_rich_patchset_custom(patchset)
    else:
        perror(f"unknown base patch set type: {type(patchset)}")
        sys.exit(errno.ENOTRECOVERABLE)

    entries = [
        f"[bold magenta]{patchset.title}[/bold magenta]",
        Group(t, rdr),
    ]

    return Group(*entries)


def _add_rich_patchset_entry(table: Table, patchset: ManifestPatchEntry) -> None:
    rdr: RenderableType
    if isinstance(patchset, PatchMeta):
        rdr = _gen_rich_patch_meta(patchset)
    elif isinstance(patchset, PatchSetBase):
        rdr = _gen_rich_patchset_base(patchset)
    else:
        perror(f"unknown patch set type: {type(patchset)}")
        sys.exit(errno.ENOTRECOVERABLE)

    row: tuple[str, str, RenderableType] = (
        str(patchset.entry_uuid),
        patchset.entry_type.value,
        rdr,
    )
    table.add_row(*row)
    pass


@click.group("patchset", help="Handle patch sets.")
def cmd_patchset() -> None:
    pass


@cmd_patchset.command("create", help="Create a patch set from individual patches.")
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
@click.option(
    "--author",
    "author_name",
    required=True,
    type=str,
    metavar="NAME",
    help="Author's name.",
)
@click.option(
    "--email",
    "author_email",
    required=True,
    type=str,
    metavar="EMAIL",
    help="Author's email.",
)
@click.option(
    "--title",
    "-T",
    "patchset_title",
    required=False,
    default="",
    type=str,
    metavar="TEXT",
    help="Title for this patch set.",
)
@click.option(
    "--desc",
    "-D",
    "patchset_desc",
    required=False,
    type=str,
    metavar="TEXT",
    help="Short description of this patch set.",
)
@click.option(
    "-r",
    "--release-name",
    "release_name",
    required=False,
    type=str,
    metavar="NAME",
    help="Release associated with this patch set.",
)
def cmd_patchset_create(
    patches_repo_path: Path,
    author_name: str,
    author_email: str,
    patchset_title: str | None,
    patchset_desc: str | None,
    release_name: str | None,
) -> None:
    print("prompt?")
    if not patchset_title:
        patchset_title = cast(
            str | None, click.prompt("Patch set title", type=str, prompt_suffix=" > ")
        )
        if not patchset_title or not patchset_title.strip():
            perror("must specify a patch set title")
            sys.exit(errno.EINVAL)

    if patchset_desc and not patchset_desc.strip():
        perror("patch set description is empty")
        sys.exit(errno.EINVAL)

    author_info = f"{author_name} <{author_email}>"
    if not patchset_desc and click.confirm("Add a description?", default=False):
        try:
            desc_msg = (
                click.edit(
                    f"{patchset_title}\n\n<description here>\n\n"
                    + f"Signed-off-by: {author_info}"
                ),
            )
        except Exception as e:
            perror(f"unable to open editor: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        print(desc_msg)
        if not desc_msg or not desc_msg[0]:
            perror("must specify a patch set description")
            sys.exit(errno.EINVAL)

        patchset_desc = desc_msg[0].strip()

    if not patchset_desc:
        patchset_desc = f"{patchset_title}\n\nSigned-off-by: {author_info}"

    patchset = CustomPatchSet(
        author=AuthorData(user=author_name, email=author_email),
        creation_date=dt.now(datetime.UTC),
        title=patchset_title,
        related_to=[],
        patches=[],
        description=patchset_desc,
        release_name=release_name,
    )

    patchset_meta_path = get_patchset_meta_path(patches_repo_path, patchset.entry_uuid)
    assert not patchset_meta_path.exists()

    try:
        write_patchset(patches_repo_path, patchset)
    except Exception as e:
        perror(f"unable to write patch set meta file: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"successfully created patch set '{patchset.entry_uuid}'")


@cmd_patchset.command("list", help="List patch sets in the patches repository.")
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
@click.option(
    "-t",
    "--type",
    "patchset_types",
    type=str,
    multiple=True,
    required=False,
    metavar="TYPE",
    help="Filter by patch set type.",
)
def cmd_patchset_list(patches_repo_path: Path, patchset_types: list[str]) -> None:
    meta_path = patches_repo_path / "ceph" / "patches" / "meta"

    avail_types = [m.value for m in ManifestPatchSetEntryType]
    if patchset_types and any(t not in avail_types for t in patchset_types):
        perror(f"unknown patch set type(s), available: {', '.join(avail_types)}")
        sys.exit(errno.EINVAL)

    table = _gen_rich_patchset_list()
    for patchset_path in meta_path.glob("*.json"):
        try:
            patchset_uuid = uuid.UUID(patchset_path.stem)
        except Exception:
            pwarn(f"malformed patch set uuid in '{patchset_path}', skip")
            continue

        try:
            patchset = load_patchset(patches_repo_path, patchset_uuid)
        except Exception as e:
            perror(f"unable to load patch set '{patchset_uuid}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        if patchset_types and patchset.entry_type.value not in patchset_types:
            continue

        _add_rich_patchset_entry(table, patchset)

    if len(table.rows) > 0:
        console.print(table)
    else:
        pwarn("no entries found")


@cmd_patchset.command("info", help="Obtain info on a given patch set.")
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
@click.option(
    "-u",
    "--patchset-uuid",
    "patchset_uuid",
    type=uuid.UUID,
    required=True,
    help="Patch set UUID.",
)
def cmd_patchset_info(patches_repo_path: Path, patchset_uuid: uuid.UUID) -> None:
    try:
        patchset = load_patchset(patches_repo_path, patchset_uuid)
    except MalformedPatchSetError as e:
        perror(f"malformed patch set '{patchset_uuid}': {e}")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load patch set '{patchset_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    console.print(_gen_rich_patchset_info(patchset))


@cmd_patchset.command("add", help="Add one or more patches to a patch set.")
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository where operations will be performed.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
@click.option(
    "-u",
    "--patchset-uuid",
    "patchset_uuid",
    type=uuid.UUID,
    required=True,
    help="Patch set UUID.",
)
@click.option(
    "--gh-repo",
    "ceph_gh_repo",
    type=str,
    required=False,
    default="ceph/ceph",
    metavar="OWNER/REPO",
    help="GitHub repository to obtain the patch(es) from.",
)
@click.option(
    "-b",
    "--branch",
    "patches_branch",
    type=str,
    required=True,
    metavar="NAME",
    help="Branch on which to find patches.",
)
@click.argument(
    "patch_sha",
    metavar="SHA|SHA1..SHA2 [...]",
    type=str,
    required=True,
    nargs=-1,
)
@pass_ctx
def cmd_patchset_add(
    ctx: Ctx,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patchset_uuid: uuid.UUID,
    ceph_gh_repo: str,
    patches_branch: str,
    patch_sha: list[str],
) -> None:
    if not ctx.github_token:
        perror("github token not specified")
        sys.exit(errno.EINVAL)

    try:
        patchset = load_patchset(patches_repo_path, patchset_uuid)
    except Exception as e:
        perror(f"unable to load patch set '{patchset_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not isinstance(patchset, CustomPatchSet):
        perror(f"patch set '{patchset_uuid}' is not a custom patch set")
        sys.exit(errno.EINVAL)

    if patchset.is_published:
        perror(f"patch set '{patchset_uuid}' is already published")
        sys.exit(errno.EINVAL)

    existing_shas = [p[0] for meta in patchset.patches_meta for p in meta.patches]

    # ensure we have the specified branch in the ceph repo, so we can actually obtain
    # the right shas
    try:
        remote = git_prepare_remote(
            ceph_repo_path, f"github.com/{ceph_gh_repo}", ceph_gh_repo, ctx.github_token
        )
    except Exception as e:
        perror(f"unable to update remote '{ceph_gh_repo}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    seq = dt.now(datetime.UTC).strftime("%Y%m%d%H%M%S")
    dst_branch = (
        f"patchset/branch/{ceph_gh_repo.replace('/', '--')}--{patches_branch}-{seq}"
    )
    try:
        _ = remote.fetch(refspec=f"{patches_branch}:{dst_branch}")
    except Exception as e:
        perror(f"unable to fetch branch '{patches_branch}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    def _cleanup() -> None:
        try:
            git_branch_delete(ceph_repo_path, dst_branch)
        except Exception as e:
            perror(f"unable to delete temporary branch '{dst_branch}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

    patches_meta_lst: list[CustomPatchMeta] = []
    skipped_patches: list[tuple[SHA, str]] = []
    for sha_entry in patch_sha:
        sha_begin: SHA
        sha_end: SHA | None = None
        if ".." in sha_entry:
            interval = sha_entry.split("..", 1)
            if len(interval) != 2 or not interval[0] or not interval[1]:
                perror(f"malformed patch interval '{sha_entry}'")
                sys.exit(errno.EINVAL)
            sha_begin, sha_end = interval[0], interval[1]
        else:
            sha_begin = sha_entry

        if not _is_valid_sha(sha_begin) or (sha_end and not _is_valid_sha(sha_end)):
            _cleanup()
            perror(f"malformed patch sha '{sha_entry}'")
            sys.exit(errno.EINVAL)

        patches_lst: list[tuple[SHA, str]] = []
        if sha_end:
            try:
                for sha, title in reversed(
                    git_patches_in_interval(ceph_repo_path, sha_begin, sha_end)
                ):
                    if sha not in existing_shas:
                        patches_lst.append((sha, title))
                    else:
                        skipped_patches.append((sha, title))
            except Exception as e:
                _cleanup()
                perror(f"unable to obtain patches in interval '{sha_entry}': {e}")
                sys.exit(errno.ENOTRECOVERABLE)
        else:
            try:
                sha, title = git_get_patch_sha_title(ceph_repo_path, sha_begin)
                if sha not in existing_shas:
                    patches_lst.append((sha, title))
                else:
                    skipped_patches.append((sha, title))
            except Exception as e:
                _cleanup()
                perror(f"unable to obtain patch info for sha '{sha_entry}': {e}")
                sys.exit(errno.ENOTRECOVERABLE)

        if patches_lst:
            patches_meta_lst.append(
                CustomPatchMeta(
                    repo=ceph_gh_repo,
                    branch=patches_branch,
                    sha=sha_begin,
                    sha_end=sha_end,
                    patches=patches_lst,
                )
            )

    if patchset.patches_meta:
        existing_patches_table = Table(
            title="Existing patches",
            title_style="bold magenta",
            show_header=False,
            show_lines=False,
            box=rich.box.HORIZONTALS,
        )
        existing_patches_table.add_column(
            justify="left", style="bold cyan", no_wrap=True
        )
        existing_patches_table.add_column(justify="left", style="white", no_wrap=False)

        for existing_entry in patchset.patches_meta:
            for sha, title in existing_entry.patches:
                existing_patches_table.add_row(sha, title)

        console.print(Padding(existing_patches_table, (1, 0, 0, 0)))

    if skipped_patches:
        skipped_patches_table = Table(
            title="Skipped patches",
            title_style="bold orange3",
            show_header=False,
            show_lines=False,
            box=rich.box.HORIZONTALS,
        )
        skipped_patches_table.add_column(
            justify="left", style="bold hot_pink", no_wrap=True
        )
        skipped_patches_table.add_column(justify="left", style="white", no_wrap=False)

        for sha, title in skipped_patches:
            skipped_patches_table.add_row(sha, title)

        console.print(Padding(skipped_patches_table, (1, 0, 0, 0)))

    if not patches_meta_lst:
        console.print(
            Padding("[bold yellow]No new patches to add[/bold yellow]", (1, 0, 1, 2))
        )
        return
    else:
        patch_lst_table = Table(
            title="Patches to add",
            title_style="bold green",
            show_header=False,
            show_lines=False,
            box=rich.box.HORIZONTALS,
        )
        patch_lst_table.add_column(justify="left", style="bold gold1", no_wrap=True)
        patch_lst_table.add_column(justify="left", style="white", no_wrap=False)

        for entry in patches_meta_lst:
            for sha, title in entry.patches:
                patch_lst_table.add_row(sha, title)

        console.print(Padding(patch_lst_table, (1, 0, 1, 0)))
        if not click.confirm("Add above patches to patch set?", prompt_suffix=" "):
            pwarn("Aborted")
            sys.exit(0)

    patchset.patches_meta.extend(patches_meta_lst)

    try:
        write_patchset(patches_repo_path, patchset)
    except Exception as e:
        _cleanup()
        perror(f"unable to write patch set meta file: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    n_patches = sum(len(meta.patches) for meta in patches_meta_lst)
    psuccess(f"wrote patch set '{patchset.entry_uuid}', {n_patches} new patches")
    _cleanup()


@cmd_patchset.command("publish", help="Publish a patch set to the patches repository.")
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to ceph git repository where operations will be performed.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
@click.option(
    "-u",
    "--patchset-uuid",
    "patchset_uuid",
    type=uuid.UUID,
    required=True,
    help="Patch set UUID.",
)
@pass_ctx
def cmd_patchset_publish(
    ctx: Ctx,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    patchset_uuid: uuid.UUID,
) -> None:
    if not ctx.github_token:
        perror("missing GitHub token")
        sys.exit(errno.EINVAL)

    try:
        patchset = load_patchset(patches_repo_path, patchset_uuid)
    except Exception as e:
        perror(f"unable to load patch set '{patchset_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not isinstance(patchset, CustomPatchSet):
        perror(f"patch set '{patchset_uuid}' is not a custom patch set")
        sys.exit(errno.EINVAL)

    if patchset.is_published:
        perror(f"patch set '{patchset_uuid}' is already published")
        sys.exit(errno.EINVAL)

    if not patchset.patches_meta or all(
        len(meta.patches) == 0 for meta in patchset.patches_meta
    ):
        perror(f"patch set '{patchset_uuid}' has no patches")
        sys.exit(errno.EINVAL)

    console.print(_gen_rich_patchset_info(patchset))
    if not click.confirm("Publish above patch set?", prompt_suffix=" "):
        pwarn("Aborted")
        sys.exit(0)

    try:
        patches = fetch_custom_patchset_patches(
            ceph_repo_path, patches_repo_path, patchset, ctx.github_token
        )
    except Exception as e:
        perror(f"unable to fetch patches for patch set '{patchset_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    patchset.patches = patches
    patchset.is_published = True

    try:
        write_patchset(patches_repo_path, patchset)
    except Exception as e:
        perror(f"unable to write patch set meta file: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"published patch set '{patchset.entry_uuid}'")


@cmd_patchset.command("remove", help="Remove an unpublished patch set.")
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
@click.option(
    "-u",
    "--patchset-uuid",
    "patchset_uuid",
    type=uuid.UUID,
    required=True,
    help="Patch set UUID.",
)
def cmd_patchset_remove(patches_repo_path: Path, patchset_uuid: uuid.UUID) -> None:
    patchset_meta_path = get_patchset_meta_path(patches_repo_path, patchset_uuid)
    if not patchset_meta_path.exists():
        perror(f"patch set '{patchset_uuid}' does not exist")
        sys.exit(errno.ENOENT)

    try:
        patchset = load_patchset(patches_repo_path, patchset_uuid)
    except Exception as e:
        perror(f"unable to load patch set '{patchset_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not isinstance(patchset, CustomPatchSet):
        perror(f"patch set '{patchset_uuid}' is not a custom patch set")
        sys.exit(errno.EINVAL)

    if patchset.is_published:
        perror(f"patch set '{patchset_uuid}' is already published")
        sys.exit(errno.EINVAL)

    console.print(_gen_rich_patchset_info(patchset))
    if not click.confirm("Remove above patch set?", prompt_suffix=" "):
        pwarn("Aborted")
        sys.exit(0)

    try:
        patchset_meta_path.unlink()
    except Exception as e:
        perror(f"unable to remove patch set meta file: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"removed patch set '{patchset.entry_uuid}'")


@cmd_patchset.group("advanced", help="Advanced patch set operations.")
def cmd_patchset_advanced() -> None:
    pass


@cmd_patchset_advanced.command(
    "migrate-store-format", help="Migrate patch sets' store format"
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
def cmd_patchset_migrate_store_format(patches_repo_path: Path) -> None:
    if not patches_repo_path.exists():
        perror(f"patches repository does not exist at '{patches_repo_path}'")
        sys.exit(errno.ENOENT)

    if not patches_repo_path.joinpath(".git").exists():
        perror("provided path for patches repository is not a git repository")
        sys.exit(errno.EINVAL)

    patches_path = patches_repo_path / "ceph" / "patches"
    if not patches_path.exists():
        pinfo(f"patches path does not exist at '{patches_path}', nothing to do")
        return

    n_patchsets = 0
    candidate_dirs: list[Path] = []
    for p in patches_path.iterdir():
        if p.is_dir() and p.name != "meta":
            candidate_dirs.append(p)

    print(candidate_dirs)
    for d in candidate_dirs:
        for p in list(d.walk()):
            for sub in p[1]:
                sub_path = Path(p[0]) / sub
                if not sub_path.is_dir():
                    continue

                if not re.match(r"^[\w\d_.-]+$", sub):
                    # not a valid repo name
                    continue

                repo_name = f"{d.name}/{sub}"

                for pr in sub_path.iterdir():
                    if pr.is_dir():
                        continue

                    if not re.match(r"^\d+$", pr.name):
                        # not a valid pr id
                        pwarn(f"skip invalid pr id '{pr.name}' in '{repo_name}'")
                        continue

                    try:
                        patchset_uuid = uuid.UUID(pr.read_text())
                    except Exception:
                        pwarn(
                            f"malformed patch set uuid in '{pr}' in '{repo_name}', skip"
                        )
                        continue

                    pinfo(f"pr id '{pr.name}' uuid '{patchset_uuid}' in '{repo_name}'")
                    latest_patchset_path = patches_path / f"{patchset_uuid}.patch"
                    latest_meta_path = patches_path / "meta" / f"{patchset_uuid}.json"

                    if (
                        not latest_patchset_path.exists()
                        and not latest_meta_path.exists()
                    ):
                        pwarn(
                            f"missing patch file '{latest_patchset_path}', "
                            + "skip migration"
                        )
                        continue

                    try:
                        patchset_meta = ManifestPatchEntryWrapper.model_validate_json(
                            latest_meta_path.read_text()
                        )
                    except pydantic.ValidationError as e:
                        perror(f"malformed meta file '{latest_meta_path}': {e}")
                        continue

                    if not isinstance(patchset_meta.contents, GitHubPullRequest):
                        perror(
                            f"found meta for patchset uuid '{patchset_uuid}' "
                            + "is not a gh pr"
                        )
                        continue

                    patchset = patchset_meta.contents
                    if not patchset.patches:
                        perror(
                            f"found empty patch set for uuid '{patchset_uuid}' "
                            + f"pr id '{pr.name}' repo '{repo_name}'"
                        )
                        continue

                    head_patch_sha = next(reversed(patchset.patches)).sha
                    pinfo(
                        f"pr id '{pr.name}' repo '{repo_name}' "
                        + f"head patch sha '{head_patch_sha}'"
                    )
                    head_path_sha_path = pr / head_patch_sha
                    latest_path = pr / "latest"

                    try:
                        pr.unlink()
                        pr.mkdir()
                        _ = head_path_sha_path.write_text(str(patchset_uuid))
                        latest_path.symlink_to(head_patch_sha)
                    except Exception as e:
                        perror(f"unable to migrate pr id '{pr.name}': {e}")
                        continue

                    psuccess(
                        f"successfully migrated pr id '{pr.name}' repo '{repo_name}'"
                    )
                    n_patchsets += 1

    psuccess(f"successfully migrated {n_patchsets} patch sets")


@cmd_patchset_advanced.command("migrate-single-patches", help="Migrate single patches.")
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True,
        dir_okay=True,
        file_okay=False,
        writable=True,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=True,
    help="Path to ces-patches git repository.",
)
def cmd_patchset_migrate_single_patches(patches_repo_path: Path) -> None:
    meta_path = patches_repo_path / "ceph" / "patches" / "meta"

    n_patchsets = 0
    for patchset_path in meta_path.glob("*.json"):
        try:
            patchset_uuid = uuid.UUID(patchset_path.stem)
        except Exception:
            pwarn(f"malformed patch set uuid in '{patchset_path}', skip")
            continue

        try:
            _ = load_patchset(patches_repo_path, patchset_uuid)
            continue
        except MalformedPatchSetError:
            # possibly a single patch, check and migrate it if so.
            pinfo(f"possible single patch '{patchset_uuid}', check")
        except Exception as e:
            perror(f"unable to load patch set '{patchset_uuid}': {e}")
            continue

        try:
            single_patch = PatchMeta.model_validate_json(patchset_path.read_text())
        except Exception:
            perror(f"unable to parse single patch meta '{patchset_path}'")
            continue

        pinfo(f"found single patch at '{single_patch.entry_uuid}', migrate")
        # backup existing meta file
        bak_path = patchset_path.with_suffix(".json.bak")
        try:
            _ = bak_path.write_text(patchset_path.read_text())
        except Exception as e:
            perror(f"unable to backup single patch meta '{patchset_path}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        try:
            _ = patchset_path.write_text(
                ManifestPatchEntryWrapper(contents=single_patch).model_dump_json(
                    indent=2
                )
            )
        except Exception as e:
            perror(f"unable to migrate single patch meta '{patchset_path}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        psuccess(f"successfully migrated single patch '{patchset_uuid}'")
        bak_path.unlink()
        n_patchsets += 1

    pinfo(f"migrated {n_patchsets} single patches")
