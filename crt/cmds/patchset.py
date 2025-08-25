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

import errno
import re
import sys
import uuid
from pathlib import Path
from typing import cast

import click
import pydantic
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
    NoSuchPatchSetError,
    PatchSetError,
)
from crtlib.github import gh_get_pr
from crtlib.manifest import load_manifest, store_manifest
from crtlib.models.discriminator import ManifestPatchEntryWrapper
from crtlib.models.patchset import GitHubPullRequest
from crtlib.patchset import (
    patchset_fetch_gh_patches,
    patchset_from_gh_needs_update,
    patchset_get_gh,
)

from cmds import Ctx, pass_ctx, perror, pinfo, psuccess, pwarn
from cmds import logger as parent_logger

logger = parent_logger.getChild("patchset")


@click.group("patchset", help="Handle patch sets.")
def cmd_patchset() -> None:
    pass


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


@cmd_patchset.command("migrate", help="Migrate patch sets' store format")
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
def cmd_patchset_migrate(patches_repo_path: Path) -> None:
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
