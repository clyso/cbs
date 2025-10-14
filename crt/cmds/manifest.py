# CBS Release Tool - manifest commands
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
import rich.box
from cbscore.versions.utils import parse_version
from crtlib.apply import ApplyConflictError, ApplyError, patches_apply_to_manifest
from crtlib.errors import CRTError
from crtlib.errors.manifest import (
    MalformedManifestError,
    ManifestError,
    NoSuchManifestError,
)
from crtlib.errors.patchset import NoSuchPatchSetError, PatchSetError
from crtlib.errors.release import NoSuchReleaseError
from crtlib.github import gh_get_pr
from crtlib.manifest import (
    ManifestExecuteResult,
    list_manifests,
    load_manifest_by_name_or_uuid,
    manifest_execute,
    manifest_exists,
    manifest_publish_branch,
    manifest_publish_stages,
    manifest_release_notes,
    remove_manifest,
    store_manifest,
)
from crtlib.models.common import ManifestPatchEntry
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patch import Patch
from crtlib.models.patchset import GitHubPullRequest
from crtlib.patchset import (
    load_patchset,
    patchset_fetch_gh_patches,
    patchset_from_gh_needs_update,
    patchset_get_gh,
)
from crtlib.release import load_release
from rich.console import Group, RenderableType
from rich.padding import Padding
from rich.panel import Panel
from rich.table import Table

from cmds._common import CRTExitError, CRTProgress, get_stage_rdr

from . import (
    Ctx,
    Symbols,
    console,
    pass_ctx,
    perror,
    pinfo,
    psuccess,
    pwarn,
    with_patches_repo_path,
)
from . import logger as parent_logger

logger = parent_logger.getChild("manifest")

_ExitError = CRTExitError


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


def _gen_rich_manifest_creation_summary(manifest: ReleaseManifest) -> Panel:
    table = _gen_rich_manifest_table(manifest)
    return Panel(
        Group(
            table,
            Padding(
                "[bold]You can now modify this release using its UUID", (1, 0, 1, 2)
            ),
        ),
        box=rich.box.SQUARE,
        title="Manifest Created",
        expand=False,
    )


@click.group("manifest", help="Operations on manifests.")
def cmd_manifest() -> None:
    pass


@cmd_manifest.command("new", help="Create a new release manifest.")
@click.option(
    "-r",
    "--release",
    "release_name",
    type=str,
    required=True,
    metavar="RELEASE",
    help="Release to create a manifest for.",
)
@click.argument("name", type=str, required=True, metavar="NAME")
@with_patches_repo_path
def cmd_manifest_new(
    patches_repo_path: Path,
    name: str,
    release_name: str,
) -> None:
    # strict check on release manifest name:
    # required prefix - ces or ccs
    # required major, minor, patch
    # required suffix - must be a development version
    try:
        v_prefix, _, v_minor, v_patch, v_suffix = parse_version(name)
    except ValueError:
        perror(f"malformed manifest name '{name}'")
        sys.exit(errno.EINVAL)

    if v_prefix not in ("ces", "ccs"):
        perror(f"manifest name '{name}' must start with 'ces' or 'ccs'")
        sys.exit(errno.EINVAL)

    if not v_minor or not v_patch or not v_suffix:
        perror("must specify major, minor, patch and suffix in manifest name")
        sys.exit(errno.EINVAL)

    try:
        release = load_release(patches_repo_path, release_name)
    except NoSuchReleaseError:
        perror(f"unable to find release '{release_name}'")
        sys.exit(errno.ENOENT)

    repo_re = re.compile(r"([\w\d_.-]+)/([\w\d_.-]+)")
    base_repo_m = re.match(repo_re, release.release_repo)
    if not base_repo_m:
        perror(f"unexpected malformed release base repository '{release.release_repo}'")
        sys.exit(errno.ENOTRECOVERABLE)

    base_repo_owner = cast(str, base_repo_m.group(1))
    base_repo_repo = cast(str, base_repo_m.group(2))

    manifest = ReleaseManifest(
        name=name,
        base_release_name=release.base_release_name,
        base_ref_org=base_repo_owner,
        base_ref_repo=base_repo_repo,
        base_ref=release.release_base_branch,
        dst_repo=release.release_repo,
    )

    try:
        store_manifest(patches_repo_path, manifest)
    except ManifestError as e:
        perror(f"unable to create manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    console.print(_gen_rich_manifest_creation_summary(manifest))


@cmd_manifest.command(
    "from", help="Create a release manifest from an existing manifest."
)
@click.argument("name_or_uuid", type=str, required=True, metavar="NAME|UUID")
@click.option(
    "--name",
    "-n",
    type=str,
    required=True,
    metavar="NAME",
    help="Name of the new release.",
)
@with_patches_repo_path
def cmd_manifest_from(patches_repo_path: Path, name_or_uuid: str, name: str) -> None:
    if manifest_exists(patches_repo_path, manifest_name=name):
        perror(f"manifest name '{name}' already exists")
        sys.exit(errno.EEXIST)

    try:
        new_manifest = load_manifest_by_name_or_uuid(patches_repo_path, name_or_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{name_or_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load manifest '{name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    old_uuid = new_manifest.release_uuid
    old_name = new_manifest.name

    new_manifest.name = name
    new_manifest.release_uuid = uuid.uuid4()
    new_manifest.creation_date = dt.now(datetime.UTC)
    new_manifest.from_name = old_name
    new_manifest.from_uuid = old_uuid

    # reset stages publish state
    for stage in new_manifest.stages:
        stage.is_published = False

    try:
        store_manifest(patches_repo_path, new_manifest)
    except ManifestError as e:
        perror(f"unable to create new manifest '{name}' from '{name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    console.print(_gen_rich_manifest_creation_summary(new_manifest))

    psuccess(
        f"created manifest name '{name}' uuid '{new_manifest.release_uuid}'\n"
        + f"   from manifest name '{old_name}' uuid '{old_uuid}'"
    )


@cmd_manifest.command("remove", help="Remove a manifest.")
@click.argument("name_or_uuid", type=str, required=True, metavar="NAME|UUID")
@click.confirmation_option(prompt="Really remove manifest?")
@with_patches_repo_path
def cmd_manifest_remove(patches_repo_path: Path, name_or_uuid: str) -> None:
    manifest_uuid: uuid.UUID | None = None
    manifest_name: str | None = None

    try:
        manifest_uuid = uuid.UUID(name_or_uuid)
    except Exception:
        manifest_name = name_or_uuid

    try:
        rm_uuid, rm_name = remove_manifest(
            patches_repo_path, manifest_uuid=manifest_uuid, manifest_name=manifest_name
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except Exception as e:
        perror(f"unable to remove manifest '{name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"removed manifest name '{rm_name}' uuid '{rm_uuid}'")


@cmd_manifest.command("list", help="List existing release manifests.")
@with_patches_repo_path
def cmd_manifest_list(patches_repo_path: Path) -> None:
    try:
        manifest_lst = list_manifests(patches_repo_path)
    except ManifestError as e:
        perror(f"unable to list manifests: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    for entry in manifest_lst:
        table = _gen_rich_manifest_table(entry)
        console.print(
            Panel(
                table,
                title=f"Manifest {entry.release_uuid}",
                box=rich.box.SQUARE,
            )
        )


@cmd_manifest.command("info", help="Show information about release manifests.")
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=False,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID for which information will be shown.",
)
@click.option(
    "-e",
    "--extended",
    "extended_info",
    is_flag=True,
    default=False,
    help="Show stage extended information.",
)
@with_patches_repo_path
def cmd_manifest_info(
    patches_repo_path: Path,
    manifest_name_or_uuid: str | None,
    extended_info: bool,
) -> None:
    manifest_lst: list[ReleaseManifest] = []

    if manifest_name_or_uuid:
        try:
            manifest_lst.append(
                load_manifest_by_name_or_uuid(patches_repo_path, manifest_name_or_uuid)
            )
        except Exception as e:
            perror(f"unable to obtain manifest '{manifest_name_or_uuid}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

    else:
        try:
            manifest_lst = list_manifests(patches_repo_path)
        except ManifestError as e:
            perror(f"unable to obtain manifest list: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

    for manifest in manifest_lst:
        table = _gen_rich_manifest_table(manifest)

        stages_renderables: list[RenderableType] = []
        for stage in manifest.stages:
            stages_renderables.append(
                get_stage_rdr(patches_repo_path, stage, extended_info=extended_info)
            )

        stages_group = (
            Group(*stages_renderables)
            if stages_renderables
            else Group("[bold red]None")
        )

        panel = Panel(
            Group(
                table,
                Padding("[red]patch sets:", (1, 0, 1, 0)),
                Padding(stages_group, (0, 0, 0, 2)),
            ),
            box=rich.box.SQUARE,
            title=f"Manifest {manifest.release_uuid}",
        )
        console.print(panel)


def _manifest_add_gh_pr(
    ceph_repo_path: Path,
    patches_repo_path: Path,
    from_gh: str,
    from_gh_repo: str,
    token: str,
    progress: CRTProgress,
) -> GitHubPullRequest:
    def _get_gh_pr_data() -> tuple[int, str, str]:
        if m := re.match(r"^(\d+)$|^https://.*/pull/(\d+).*$", from_gh):
            pr_id = int(m.group(1))
        else:
            perror("malformed GitHub pull request ID or URL")
            sys.exit(errno.EINVAL)

        if m := re.match(r"^([\w\d_.-]+)/([\w\d_.-]+)$", from_gh_repo):
            gh_owner = cast(str, m.group(1))
            gh_repo = cast(str, m.group(2))
        else:
            perror("malformed GitHub repository name")
            raise _ExitError(errno.EINVAL)

        if from_gh and not from_gh_repo:
            perror("missing GitHub repository to obtain patch set from")
            raise _ExitError(errno.EINVAL)

        return (pr_id, gh_owner, gh_repo)

    gh_pr_id, gh_repo_owner, gh_repo = _get_gh_pr_data()
    logger.debug(f"obtain gh pr {gh_repo_owner}/{gh_repo}#{gh_pr_id}")

    task_gh_pr_str = f"{gh_repo_owner}/{gh_repo}#{gh_pr_id}"

    progress.new_task(
        f"obtaining pull request info from {task_gh_pr_str}",
    )

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
        raise _ExitError(errno.ENOTRECOVERABLE) from e
    except Exception as e:
        perror(f"error found: {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from e

    if existing_patchset:
        if not existing_patchset.merged:
            pinfo("update patch set from github")
            update_from_gh = True
        else:
            patchset = existing_patchset

    if needs_patchset or update_from_gh:
        # obtain from github
        try:
            patchset = gh_get_pr(gh_repo_owner, gh_repo, gh_pr_id, token=token)
        except CRTError as e:
            perror(f"unable to obtain pull request info from github: {e}")
            raise _ExitError(e.ec if e.ec else errno.ENOTRECOVERABLE) from e

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

    progress.done_task()
    progress.new_task(f"fetch patch set for {task_gh_pr_str}")

    if needs_patchset:
        try:
            patchset_fetch_gh_patches(
                ceph_repo_path,
                patches_repo_path,
                patchset,
                token,
                force=force_update,
            )
        except PatchSetError as e:
            perror(f"unable to obtain patch set: {e}")
            raise _ExitError(errno.ENOTRECOVERABLE) from e
        except Exception as e:
            perror(f"unexpected error: {e}")
            raise _ExitError(errno.ENOTRECOVERABLE) from e

    progress.done_task()
    return patchset


def _manifest_add_patchset_by_uuid(
    patches_repo_path: Path, patchset_uuid: uuid.UUID
) -> ManifestPatchEntry:
    try:
        patchset = load_patchset(patches_repo_path, patchset_uuid)
    except NoSuchPatchSetError:
        perror(f"patch set uuid '{patchset_uuid}' not found")
        raise _ExitError(errno.ENOENT) from None
    except Exception as e:
        perror(f"unable to load patch set '{patchset_uuid}': {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from e

    return patchset


@cmd_manifest.command("add", help="Add a patch set to a release.")
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
    envvar="CRT_CEPH_REPO_PATH",
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
    "-P",
    "--patchset-uuid",
    "patchset_uuid",
    required=False,
    type=str,
    metavar="UUID",
    help="Specify existing patch set UUID to add to the manifest.",
)
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=True,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID to which the patch set will be added.",
)
@with_patches_repo_path
@pass_ctx
def cmd_manifest_add_patchset(
    ctx: Ctx,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    from_gh: str | None,
    from_gh_repo: str | None,
    patchset_uuid: uuid.UUID | None,
    manifest_name_or_uuid: str,
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

    _check_repo(patches_repo_path, "patches")
    _check_repo(ceph_repo_path, "ceph")

    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_name_or_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not manifest.active_stage:
        perror(f"manifest uuid '{manifest_name_or_uuid}' has no active stage")
        pwarn("please run '[bold bright_magenta]stage new[/bold bright_magenta]'")
        sys.exit(errno.ENOENT)

    progress = CRTProgress(console)
    progress.start()

    patchset: ManifestPatchEntry
    if from_gh:
        if not from_gh_repo:
            perror("missing GitHub repository to obtain patch set from")
            sys.exit(errno.EINVAL)

        if patchset_uuid:
            perror("cannot specify both --from-gh and --patchset-uuid")
            sys.exit(errno.EINVAL)

        progress.new_task("adding from github pull request")

        try:
            patchset = _manifest_add_gh_pr(
                ceph_repo_path,
                patches_repo_path,
                from_gh,
                from_gh_repo,
                ctx.github_token,
                progress,
            )
        except _ExitError as e:
            progress.stop_error()
            sys.exit(e.code)
        except Exception as e:
            perror(f"unable to add patch set from github: {e}")
            progress.stop_error()
            sys.exit(errno.ENOTRECOVERABLE)

        progress.done_task()

    elif patchset_uuid:
        progress.new_task(f"adding from patch set '{patchset_uuid}'")
        try:
            patchset = _manifest_add_patchset_by_uuid(patches_repo_path, patchset_uuid)
        except _ExitError as e:
            progress.stop_error()
            sys.exit(e.code)
        except Exception as e:
            progress.stop_error()
            perror(f"unable to add patch set '{patchset_uuid}': {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        progress.done_task()

    else:
        perror("either --from-gh or --patchset-uuid must be specified")
        progress.stop()
        sys.exit(errno.EINVAL)

    if manifest.contains_patchset(patchset):
        progress.stop()
        pinfo(f"manifest '{manifest_name_or_uuid}' already contains {patchset.repr}")
        return

    pinfo("apply patch set to manifest's repository")
    progress.new_task("applying patch set to manifest")
    try:
        _, added, skipped = patches_apply_to_manifest(
            manifest, patchset, ceph_repo_path, patches_repo_path, ctx.github_token
        )
    except (ApplyError, Exception) as e:
        perror(f"unable to apply to manifest: {e}")
        progress.stop_error()
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()

    logger.debug(f"added: {added}")
    logger.debug(f"skipped: {skipped}")
    psuccess("successfully applied patch set to manifest")

    if not manifest.add_patches(patchset):
        perror("unexpected error adding patch set to manifest !!")
        progress.stop()
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest '{manifest_name_or_uuid}' to db: {e}")
        progress.stop()
        sys.exit(errno.ENOTRECOVERABLE)

    progress.stop()

    psuccess(f"patch set {patchset.repr} added to manifest '{manifest_name_or_uuid}'")


def _manifest_execute(
    manifest: ReleaseManifest,
    *,
    token: str,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    no_cleanup: bool = True,
    progress: CRTProgress,
) -> tuple[ManifestExecuteResult, RenderableType]:
    """
    Execute a manifest and return a renderable for the console.

    This function is shared between 'validate' and 'publish'.
    """
    progress.new_task("executing manifest")

    try:
        res = manifest_execute(
            manifest, ceph_repo_path, patches_repo_path, token, no_cleanup=no_cleanup
        )
    except ApplyConflictError as e:
        progress.stop_error()
        perror(f"{len(e.conflict_files)} file conflicts found applying manifest")
        pinfo(f"[bold]on sha '{e.sha}':[/bold]")
        for file in e.conflict_files:
            pinfo(f"{Symbols.SMALL_RIGHT_ARROW} {file}")
        sys.exit(errno.EAGAIN)

    except ApplyError as e:
        perror(f"unable to apply manifest: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    progress.done_task()

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
    ceph_repo_path: Path,
    patches_repo_path: Path,
    manifest: ReleaseManifest,
    our_branch: str,
    branch_prefix: str,
    progress: CRTProgress,
) -> RenderableType:
    progress.new_task("publishing")

    progress.new_task("publishing manifest stages")
    try:
        n_patches = manifest_publish_stages(patches_repo_path, manifest)
    except ManifestError as e:
        progress.error_task()
        perror(f"unable to publish manifest stages: {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from None

    logger.info(f"published {n_patches} patches for manifest")

    progress.done_task()

    progress.new_task("publish branch")
    dst_branch = f"{branch_prefix}/{manifest.name}"
    try:
        res = manifest_publish_branch(
            manifest,
            ceph_repo_path,
            our_branch,
            dst_branch,
        )
    except ManifestError as e:
        progress.error_task()
        perror(f"unable to publish manifest '{manifest.release_uuid}': {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from None

    logger.debug(f"set manifest '{manifest.name}' dest branch to '{dst_branch}'")
    manifest.dst_branch = dst_branch
    try:
        store_manifest(patches_repo_path, manifest)
    except ManifestError as e:
        progress.error_task()
        perror(f"unable to store manifest after publishing: {e}")
        raise _ExitError(errno.ENOTRECOVERABLE) from None

    progress.done_task()
    progress.done_task()

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
@click.argument("manifest_name_or_uuid", type=str, required=True, metavar="NAME|UUID")
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    envvar="CRT_CEPH_REPO_PATH",
    required=True,
    help="Path to ceph git repository.",
)
@click.option(
    "--no-cleanup",
    is_flag=True,
    default=False,
    show_default=True,
    help="Whether to clean up after validation.",
)
@with_patches_repo_path
@pass_ctx
def cmd_manifest_validate(
    ctx: Ctx,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    manifest_name_or_uuid: str,
    no_cleanup: bool,
) -> None:
    logger.info(f"apply manifest '{manifest_name_or_uuid}' to repo '{ceph_repo_path}'")

    if not ctx.github_token:
        perror("missing github token")
        sys.exit(errno.EINVAL)

    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    progress = CRTProgress(console)
    progress.start()
    (_, apply_summary) = _manifest_execute(
        manifest,
        token=ctx.github_token,
        ceph_repo_path=ceph_repo_path,
        patches_repo_path=patches_repo_path,
        no_cleanup=no_cleanup,
        progress=progress,
    )
    progress.stop()

    panel = Panel(
        apply_summary,
        box=rich.box.SQUARE,
        title=f"Manifest {manifest.release_uuid}",
    )
    console.print(panel)


@cmd_manifest.command("publish")
@click.argument("manifest_name_or_uuid", type=str, required=True, metavar="NAME|UUID")
@click.option(
    "-c",
    "--ceph-repo",
    "ceph_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    envvar="CRT_CEPH_REPO_PATH",
    help="Path to ceph git repository.",
)
@click.option(
    "--prefix",
    "release_branch_prefix",
    type=str,
    metavar="PREFIX",
    required=False,
    default="release-dev",
    help="Prefix to use for published branch.",
)
@with_patches_repo_path
@pass_ctx
def cmd_manifest_publish(
    ctx: Ctx,
    patches_repo_path: Path,
    ceph_repo_path: Path,
    release_branch_prefix: str,
    manifest_name_or_uuid: str,
) -> None:
    """
    Publish a manifest.

    Will upload the manifest to S3, and push a branch to the specified repository.
    """
    logger.info(f"publish manifest '{manifest_name_or_uuid}'")

    if not ctx.github_token:
        perror("missing github token")
        sys.exit(errno.EINVAL)

    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if all(s.is_published for s in manifest.stages):
        perror(f"manifest '{manifest_name_or_uuid}' is already published")
        sys.exit(errno.EALREADY)

    progress = CRTProgress(console)
    progress.start()

    execute_res, execute_summary = _manifest_execute(
        manifest,
        token=ctx.github_token,
        ceph_repo_path=ceph_repo_path,
        patches_repo_path=patches_repo_path,
        no_cleanup=True,
        progress=progress,
    )

    try:
        publish_summary = _manifest_publish(
            ceph_repo_path,
            patches_repo_path,
            manifest,
            execute_res.target_branch,
            release_branch_prefix,
            progress,
        )
    except _ExitError as e:
        progress.stop_error()
        sys.exit(e.code)

    progress.stop()

    console.print(
        Padding(
            Panel(
                Group(
                    Padding(execute_summary, (0, 0, 1, 0)),
                    Padding(publish_summary, (0, 0, 1, 0)),
                ),
                title=f"Manifest {manifest.release_uuid}",
                box=rich.box.SQUARE,
            ),
            (1, 0, 0, 0),
        )
    )


@click.command("release-notes", help="Automatically generate release notes.")
@click.argument("name_or_uuid", type=str, required=True, metavar="NAME|UUID")
@click.option(
    "--cephadm-loc",
    "cephadm_loc",
    type=str,
    required=False,
    help="Location (URL) of cephadm binary.",
)
@click.option(
    "--image-loc",
    "image_loc",
    type=str,
    required=False,
    help="Location (URL) of ceph container image.",
)
@click.option(
    "--stdout",
    "to_stdout",
    is_flag=True,
    default=False,
    help="Only print release notes to stdout.",
)
@with_patches_repo_path
def cmd_manifest_release_notes(
    patches_repo_path: Path,
    name_or_uuid: str,
    cephadm_loc: str | None,
    image_loc: str | None,
    to_stdout: bool,
) -> None:
    try:
        manifest = load_manifest_by_name_or_uuid(patches_repo_path, name_or_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest '{name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{name_or_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to load manifest '{name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    txt = manifest_release_notes(manifest, image_loc=image_loc, cephadm_loc=cephadm_loc)
    if to_stdout:
        print(txt)

    dst_path = patches_repo_path / "release-notes" / f"{manifest.name}.md"
    if dst_path.exists() and not click.confirm(
        "Release notes file exists. Overwrite?", default=False, prompt_suffix=""
    ):
        pinfo(f"not writing release notes to {dst_path}")
        sys.exit(0)

    dst_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        _ = dst_path.write_text(txt, encoding="utf-8")
    except Exception as e:
        perror(f"unable to write release notes to '{dst_path}': {e}")
        sys.exit(errno.EIO)

    psuccess(f"wrote release notes to '{dst_path}'")


@cmd_manifest.group("advanced", help="Advanced manifest operations.")
def cmd_manifest_advanced() -> None:
    pass


@cmd_manifest_advanced.command(
    "manifest-update", help="Update the manifest on-disk representation."
)
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=False,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID to update.",
)
@with_patches_repo_path
def cmd_manifest_update(patches_repo_path: Path, manifest_name_or_uuid: str) -> None:
    pwarn(f"updating on-disk representation of manifest '{manifest_name_or_uuid}'")

    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except Exception as e:
        perror(f"unable to load manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to store manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"updated manifest '{manifest_name_or_uuid}' on-disk representation")
