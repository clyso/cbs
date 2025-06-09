# crt - utils
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

import re
from pathlib import Path
from typing import cast, override

import git
from crtlib.errors import CRTError
from crtlib.logger import logger as parent_logger

logger = parent_logger.getChild("git")


SHA = str


class GitError(CRTError):
    @override
    def __str__(self) -> str:
        return "git error" + (f": {self.msg}" if self.msg else "")


class GitPatchDiffError(GitError):
    @override
    def __str__(self) -> str:
        return "patches diff error" + (f": {self.msg}" if self.msg else "")


class GitEmptyPatchDiffError(GitPatchDiffError):
    pass


class GitCherryPickError(GitError):
    @override
    def __str__(self) -> str:
        return "cherry-pick error" + (f": {self.msg}" if self.msg else "")


class GitCherryPickConflictError(GitCherryPickError):
    sha: SHA
    conflicts: list[str]

    def __init__(self, sha: SHA, files: list[str] | None = None) -> None:
        super().__init__(msg=f"conflict occurred on sha '{sha}'")
        self.sha = sha
        self.conflicts = files if files else []


def git_check_patches_diff(
    ceph_git_path: Path,
    upstream_ref: str | SHA,
    head_ref: str | SHA,
    *,
    limit: str | SHA | None = None,
) -> tuple[list[str], list[str]]:
    logger.debug(
        f"check ref '{head_ref}' against upstream '{upstream_ref}', limit '{limit}'"
    )
    repo = git.Repo(ceph_git_path)

    cmd = ["git", "cherry", upstream_ref, head_ref]
    if limit:
        cmd.append(limit)

    try:
        res = repo.git.execute(
            cmd,
            with_extended_output=False,
            as_process=False,
            stdout_as_string=True,
        )
    except Exception as e:
        msg = (
            f"unable to check patch diff between '{upstream_ref}' and '{head_ref}': {e}"
        )
        logger.error(msg)
        raise GitPatchDiffError(msg=msg) from None

    if not res:
        logger.warning(f"empty diff between '{upstream_ref}' and '{head_ref}")
        raise GitEmptyPatchDiffError()

    patches_res = res.splitlines()
    patches_add: list[str] = []
    patches_drop: list[str] = []

    entry_re = re.compile(r"^([-+])\s+(.*)$")
    for entry in patches_res:
        m = re.match(entry_re, entry)
        if not m:
            logger.error(f"unexpected entry format: {entry}")
            continue

        action = cast(str, m.group(1))
        sha = cast(str, m.group(2))

        match action:
            case "+":
                patches_add.append(sha)
            case "-":
                patches_drop.append(sha)
            case _:
                logger.error(f"unexpected patch action '{action}' for sha '{sha}'")

    logger.debug(f"ref '{head_ref}' add {patches_add}")
    logger.debug(f"ref '{head_ref}' drop {patches_drop}")

    return (patches_add, patches_drop)


def git_status(repo_path: Path) -> list[tuple[str, str]]:
    repo = git.Repo(repo_path)

    try:
        res = cast(str, repo.git.status(["--porcelain"]))  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to run git status on '{repo_path}'"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitError(msg=msg) from None

    status_lst: list[tuple[str, str]] = []
    for entry in res.splitlines():
        status, file = entry.split()
        status_lst.append((status, file))

    return status_lst


def git_cherry_pick(repo_path: Path, sha: SHA) -> None:
    repo = git.Repo(repo_path)

    try:
        repo.git.cherry_pick(["-x", "-s", sha])  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to cherry-pick patch sha '{sha}'"
        logger.error(msg)

        status_files = git_status(repo_path)
        conflicts: list[str] = [f for s, f in status_files if s == "UU"]

        if conflicts:
            raise GitCherryPickConflictError(sha, conflicts) from None

        logger.error(e.stderr)
        raise GitCherryPickError(msg=msg) from None


def git_abort_cherry_pick(repo_path: Path) -> None:
    repo = git.Repo(repo_path)

    try:
        _ = repo.git.cherry_pick("--abort")  # pyright: ignore[reportAny]
    except git.CommandError as e:
        logger.error(f"found error aborting cherry-pick: {e.stderr}")
