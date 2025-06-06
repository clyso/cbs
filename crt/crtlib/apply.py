# crt - apply manifest
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
import re
import sys
from datetime import datetime as dt
from pathlib import Path
from typing import cast, override

import git
from crtlib.db import ReleasesDB
from crtlib.logger import logger as parent_logger
from crtlib.manifest import ReleaseManifest
from crtlib.patchset import GitHubPullRequest, patchset_check_patches

logger = parent_logger.getChild("apply")


class ApplyError(Exception):
    msg: str | None

    def __init__(self, *, msg: str | None = None) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return "error applying manifest" + (f": {self.msg}" if self.msg else "")


def apply_manifest(
    db: ReleasesDB, manifest: ReleaseManifest, ceph_git_path: Path, token: str
) -> None:
    # start new branch to apply manifest to.
    seq = dt.now(datetime.UTC).strftime("%Y%m%dT%H%M%S")
    branch_name = f"{manifest.name}-{manifest.release_git_uid}-{seq}"

    repo = git.Repo(ceph_git_path)

    logger.debug(f"branch: {branch_name}")

    def _check_repo() -> None:
        for what in ["name", "email"]:
            try:
                res = repo.git.execute(
                    ["git", "config", f"user.{what}"],
                    with_extended_output=False,
                    as_process=False,
                    stdout_as_string=True,
                )
            except Exception:
                msg = f"error obtaining repository's user's {what}"
                logger.exception(msg)
                raise ApplyError(msg=msg) from None

            if not res:
                msg = f"user's {what} not set for repository"
                logger.error(msg)
                raise ApplyError(msg=msg)

    def _prepare_remote(org: str, repo_name: str) -> git.Remote:
        remote_name = f"{org}/{repo_name}"
        try:
            remote = repo.remote(remote_name)
        except ValueError:
            remote_url = (
                f"https://ceph-release-tool:{token}@github.com/{org}/{repo_name}"
            )
            remote = repo.create_remote(remote_name, remote_url)
            logger.debug(f"create remote name: {remote_name}, url: {remote_url}")

        logger.debug("update remote")
        _ = remote.update()
        logger.debug("update submodules")
        repo.git.execute(  # pyright: ignore[reportCallIssue]
            ["git", "submodule", "update", "--init", "--recursive"],
            output_stream=sys.stdout.buffer,
            as_process=False,
            with_stdout=True,
        )
        return remote

    def _checkout_base_ref() -> git.Head:
        assert branch_name not in repo.heads
        try:
            new_head = repo.create_head(branch_name, manifest.base_ref)
        except Exception:
            msg = (
                f"unable to create new head '{branch_name}' "
                + f"from '{manifest.base_ref}'"
            )
            logger.exception(msg)
            raise ApplyError(msg=msg) from None

        repo.head.reference = new_head
        _ = repo.head.reset(index=True, working_tree=True)
        return new_head

    def _prepare_patchsets() -> list[GitHubPullRequest]:
        patchset_lst: list[GitHubPullRequest] = []
        for patchset_uuid in manifest.patchsets:
            try:
                patchset = db.load_patchset(patchset_uuid)
            except Exception as e:
                raise e from None

            if not isinstance(patchset, GitHubPullRequest):
                continue

            patchset_lst.append(patchset)
            remote = _prepare_remote(patchset.org_name, patchset.repo_name)
            pr_id = patchset.pull_request_id
            src_ref = f"pull/{pr_id}/head"
            dst_ref = f"patchset/gh/{patchset.org_name}/{patchset.repo_name}/{pr_id}"
            _ = remote.fetch(f"{src_ref}:{dst_ref}")

            _ = patchset_check_patches(
                ceph_git_path, patchset, dst_ref, manifest.base_ref
            )

        return patchset_lst

    def _check_patch_needed(sha: str) -> int:
        try:
            res = repo.git.execute(
                ["git", "cherry", "HEAD", sha, f"{sha}~1"],
                with_extended_output=False,
                as_process=False,
                stdout_as_string=True,
            )
        except Exception:
            msg = f"unable to check patch diff between HEAD and sha '{sha}'"
            logger.error(msg)
            raise ApplyError(msg=msg) from None

        if not res:
            logger.warning(f"empty diff between HEAD and sha '{sha}")
            return 0

        patches_res = res.splitlines()
        if len(patches_res) != 1:
            logger.warning(
                f"potential wrong base ref '{manifest.base_ref}' for patch sha '{sha}'"
            )
            return 0

        m = re.match(r"^([-+])\s+(.*)$", patches_res[0])
        if not m:
            logger.error(f"unexpected entry format: {patches_res[0]}")
            return 0

        action = cast(str, m.group(1))
        sha = cast(str, m.group(2))

        match action:
            case "+":
                return 1
            case "-":
                return -1
            case _:
                logger.error(f"unexpected patch action '{action}' for sha '{sha}'")
                return 0

    def _apply_patchsets(patchsets: list[GitHubPullRequest]) -> None:
        for patchset in patchsets:
            logger.debug(
                f"apply patch set uuid '{patchset.patchset_uuid}', "
                + f"pr id '{patchset.pull_request_id}'"
            )
            for patch in patchset.patches:
                logger.debug(f"apply patch uuid '{patch.patch_uuid}' sha '{patch.sha}'")

                if _check_patch_needed(patch.sha) <= 0:
                    logger.info(
                        f"patch uuid '{patch.patch_uuid}' sha '{patch.sha}' skipped"
                    )
                    continue

                try:
                    repo.git.cherry_pick("-x", "-s", patch.sha)  # pyright: ignore[reportAny]
                except git.CommandError:
                    msg = (
                        f"unable to cherry-pick uuid '{patch.patch_uuid}' "
                        + f"sha '{patch.sha}'"
                    )
                    logger.error(msg)
                    raise ApplyError(msg=msg) from None
        pass

    logger.debug("check repository requirements")
    _check_repo()
    logger.debug("prepare remote")
    _remote = _prepare_remote(manifest.base_ref_org, manifest.base_ref_repo)
    logger.debug("checkout branch")
    _branch = _checkout_base_ref()
    logger.debug("prepare patch sets")
    patchsets = _prepare_patchsets()
    logger.debug("apply patches")
    _apply_patchsets(patchsets)
