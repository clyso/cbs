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
from crtlib.apply import (
    ApplyError,
    patches_apply_to_manifest,
)
from crtlib.errors import CRTError
from crtlib.errors.manifest import (
    MalformedManifestError,
    NoSuchManifestError,
)
from crtlib.errors.patchset import (
    MalformedPatchSetError,
    NoSuchPatchSetError,
    PatchSetError,
)
from crtlib.github import gh_get_pr
from crtlib.manifest import load_manifest, store_manifest
from crtlib.models.common import AuthorData, ManifestPatchEntry
from crtlib.models.discriminator import ManifestPatchEntryWrapper
from crtlib.models.patch import PatchMeta
from crtlib.models.patchset import CustomPatchSet, GitHubPullRequest, PatchSetBase
from crtlib.patchset import (
    load_patchset,
    patchset_fetch_gh_patches,
    patchset_from_gh_needs_update,
    patchset_get_gh,
)
from rich.console import Group, RenderableType
from rich.table import Table

from cmds import Ctx, console, pass_ctx, perror, pinfo, psuccess, pwarn
from cmds import logger as parent_logger

logger = parent_logger.getChild("patchset")


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
    t1.add_row(
        f"[italic]release:[/italic] [orange3]{release}[/orange3]",
        f"[italic]published:[/italic] {is_published}",
        f"[italic]patches:[/italic] [orange3]{len(patchset.patches)}[/orange3]",
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

    patchset_meta_path = (
        patches_repo_path / "ceph" / "patches" / "meta" / f"{patchset.entry_uuid}.json"
    )
    assert not patchset_meta_path.exists()
    patchset_meta_path.parent.mkdir(parents=True, exist_ok=True)

    try:
        _ = patchset_meta_path.write_text(
            ManifestPatchEntryWrapper(contents=patchset).model_dump_json(indent=2)
        )
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
def cmd_patchset_list(patches_repo_path: Path) -> None:
    meta_path = patches_repo_path / "ceph" / "patches" / "meta"

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

        _add_rich_patchset_entry(table, patchset)

    console.print(table)
    pass


# TODO: move this command to 'manifest', and make it just be 'add'. We'll need
# 'patchset add' to add patches to a patchset.
@cmd_patchset.command("add", help="Add a new patch set to a release.")
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
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
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
    help="Path to the staging ceph git repository.",
)
@click.option(
    "--from-gh",
    type=str,
    required=False,
    metavar="PR_ID|URL",
    help="From a GitHub pull request.",
)
@click.option(
    "--from-gh-repo",
    type=str,
    required=False,
    metavar="OWNER/REPO",
    default="ceph/ceph",
    help="Specify GitHub repository to obtain patch set from",
    show_default=True,
)
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to which the patch set will be added.",
)
@pass_ctx
def cmd_patchset_add(
    ctx: Ctx,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    from_gh: str | None,
    from_gh_repo: str | None,
    manifest_uuid: uuid.UUID,
) -> None:
    if not ctx.github_token:
        perror("missing GitHub token")
        sys.exit(errno.EINVAL)

    def _check_repo(repo_path: Path, what: str) -> None:
        if not repo_path.exists():
            perror(f"{what} repository does not exist at '{repo_path}'")
            sys.exit(errno.ENOENT)

        if not repo_path.joinpath(".git").exists():
            perror(f"provided path for {what} repository is not a git repository")
            sys.exit(errno.EINVAL)

    def _get_gh_pr_data() -> tuple[int | None, str | None, str | None]:
        pr_id: int | None = None
        if from_gh:
            if m := re.match(r"^(\d+)$|^https://.*/pull/(\d+).*$", from_gh):
                pr_id = int(m.group(1))
            else:
                perror("malformed GitHub pull request ID or URL")
                sys.exit(errno.EINVAL)

        gh_owner: str | None = None
        gh_repo: str | None = None
        if from_gh_repo:
            if m := re.match(r"^([\w\d_.-]+)/([\w\d_.-]+)$", from_gh_repo):
                gh_owner = cast(str, m.group(1))
                gh_repo = cast(str, m.group(2))
            else:
                perror("malformed GitHub repository name")
                sys.exit(errno.EINVAL)

        if from_gh and not from_gh_repo:
            perror("missing GitHub repository to obtain patch set from")
            sys.exit(errno.EINVAL)

        return (pr_id, gh_owner, gh_repo)

    _check_repo(patches_repo_path, "patches")
    _check_repo(ceph_repo_path, "ceph")
    gh_pr_id, gh_repo_owner, gh_repo = _get_gh_pr_data()

    try:
        manifest = load_manifest(patches_repo_path, manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not manifest.active_stage:
        perror(f"manifest uuid '{manifest_uuid}' has no active stage")
        pwarn("please run '[bold bright_magenta]stage new[/bold bright_magenta]'")
        sys.exit(errno.ENOENT)

    if not gh_pr_id:
        # FIXME: for now, we don't deal with anything other than gh patch sets
        pwarn("not currently supported")
        return

    # FIXME: this must be properly checked once we support more than just gh prs
    assert gh_repo_owner
    assert gh_repo

    needs_patchset = False
    update_from_gh = False
    patchset: GitHubPullRequest | None = None
    existing_patchset: GitHubPullRequest | None = None
    try:
        existing_patchset = patchset_get_gh(
            patches_repo_path, gh_repo_owner, gh_repo, gh_pr_id
        )
        pinfo("found patch set")
    except NoSuchPatchSetError:
        pinfo("patch set not found, obtain from github")
        needs_patchset = True
    except PatchSetError as e:
        perror(f"unable to obtain patch set: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        perror(f"error found: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if existing_patchset:
        if not existing_patchset.merged:
            pinfo("update patch set from github")
            update_from_gh = True
        else:
            patchset = existing_patchset

    if needs_patchset or update_from_gh:
        # obtain from github
        try:
            patchset = gh_get_pr(
                gh_repo_owner, gh_repo, gh_pr_id, token=ctx.github_token
            )
        except CRTError as e:
            perror(f"unable to obtain pull request info from github: {e}")
            sys.exit(e.ec if e.ec else errno.ENOTRECOVERABLE)

    assert patchset

    force_update = False
    if update_from_gh:
        assert existing_patchset

        if patchset_from_gh_needs_update(existing_patchset, patchset):
            pinfo("patch set needs update, will update")
            needs_patchset = True
            force_update = True
        else:
            pinfo("patch set is up-to-date with github, don't fetch")
            needs_patchset = False
            # ensure we use the existing patchset instead of whatever we obtained from
            # gh -- otherwise we'll be looking for a patch set that does not exist on
            # disk, given we'd be using a "new" patch set that we'll not actually
            # obtain.
            patchset = existing_patchset

    if needs_patchset:
        try:
            patchset_fetch_gh_patches(
                ceph_repo_path,
                patches_repo_path,
                patchset,
                ctx.github_token,
                force=force_update,
            )
        except PatchSetError as e:
            perror(f"unable to obtain patch set: {e}")
            sys.exit(errno.ENOTRECOVERABLE)
        except Exception as e:
            perror(f"unexpected error: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

    if manifest.contains_patchset(patchset):
        _pr_id = f"{gh_repo_owner}/{gh_repo}#{gh_pr_id}"
        pinfo(f"manifest '{manifest_uuid}' already contains pr '{_pr_id}'")
        return

    pinfo("apply patch set to manifest's repository")
    try:
        _, added, skipped = patches_apply_to_manifest(
            manifest, patchset, ceph_repo_path, patches_repo_path, ctx.github_token
        )
    except (ApplyError, Exception) as e:
        perror(f"unable to apply to manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    logger.debug(f"added: {added}")
    logger.debug(f"skipped: {skipped}")
    psuccess("successfully applied patch set to manifest")

    if not manifest.add_patches(patchset):
        perror("unexpected error adding patch set to manifest !!")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"pr id '{gh_pr_id}' added to manifest '{manifest_uuid}'")


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
