# CES library - git utilities
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
from pathlib import Path
from typing import override

from ceslib.errors import CESError
from ceslib.utils import CmdArgs, MaybeSecure, async_run_cmd
from ceslib.utils import logger as parent_logger

logger = parent_logger.getChild("git")


class GitError(CESError):
    retcode: int

    def __init__(self, retcode: int, msg: str) -> None:
        super().__init__(msg)
        self.retcode = retcode

    @override
    def __str__(self) -> str:
        return f"git error: {self.msg} (retcode: {self.retcode})"


class GitConfigNotSetError(GitError):
    def __init__(self, what: str) -> None:
        super().__init__(errno.ENOENT, f"{what} not set in config")


async def run_git(args: CmdArgs, *, path: Path | None = None) -> str:
    """
    Run a git command within the repository.

    If `path` is provided, run the command in `path`. Otherwise, run in the current
    directory.
    """
    cmd: CmdArgs = ["git"]
    if path is not None:
        cmd.extend(["-C", path.resolve().as_posix()])

    cmd.extend(args)
    logger.debug(f"run {cmd}")
    try:
        rc, stdout, stderr = await async_run_cmd(cmd)
    except Exception as e:
        msg = f"unexpected error running command: {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e

    if rc != 0:
        logger.error(f"unable to obtain result from git '{args}': {stderr}")
        raise GitError(rc, stderr)

    return stdout


async def get_git_user() -> tuple[str, str]:
    """Obtain the current repository's git user and email, returned as a tuple."""

    async def _run_git_config_for(v: str) -> str:
        val = await run_git(["config", v])
        if len(val) == 0:
            logger.error(f"'{v}' not set in git config")
            raise GitConfigNotSetError(v)

        return val.strip()

    user_name = await _run_git_config_for("user.name")
    user_email = await _run_git_config_for("user.email")
    assert len(user_name) > 0 and len(user_email) > 0
    return (user_name, user_email)


async def get_git_repo_root() -> Path:
    """Obtain the root of the current git repository."""
    val = await run_git(["rev-parse", "--show-toplevel"])
    if len(val) == 0:
        logger.error("unable to obtain toplevel git directory path")
        raise GitError(errno.ENOENT, "top-level git directory not found")

    return Path(val.strip())


async def get_git_modified_paths(
    base_sha: str,
    ref: str,
    *,
    in_repo_path: str | None = None,
    repo_path: Path | None = None,
) -> tuple[list[Path], list[Path]]:
    """
    Obtain all modifications since `ref` on the repository.

    If `path` is specified, perform the action within the context of `path`. Otherwise,
    on the git repository existing in current directory.
    """
    try:
        cmd: CmdArgs = [
            "diff-tree",
            "--diff-filter=ACDMR",
            "--ignore-all-space",
            "--no-commit-id",
            "--name-status",
            "-r",
            base_sha,
            ref,
        ]

        if in_repo_path:
            cmd.extend(["--", in_repo_path])

        val = await run_git(cmd, path=repo_path)
    except GitError as e:
        logger.exception("error: unable to obtain latest patch")
        raise GitError(
            errno.ENOTRECOVERABLE,
            f"unable to obtain patches between {base_sha} and {ref}",
        ) from e

    if len(val) == 0:
        logger.debug(f"no relevant patches found between {base_sha} and {ref}")
        return [], []

    descs_deleted: list[Path] = []
    descs_modified: list[Path] = []

    lines = val.splitlines()
    regex = (
        re.compile(rf"^\s*([ACDMR])\s+({repo_path}.*)\s*$")
        if repo_path is not None
        else re.compile(r"\s*([ACDMR])\s+([^\s]+)\s*$")
    )
    for line in lines:
        m = re.match(regex, line)
        if m is None:
            logger.debug(f"'{line}' does not match")
            continue

        action = m.group(1)
        target = m.group(2)
        logger.debug(f"action: {action}, target: {target}")

        match action:
            case "D":
                descs_deleted.append(Path(target))
            case "A" | "C" | "M" | "R":
                descs_modified.append(Path(target))
            case _:
                logger.error(
                    f"unexpected action '{action}' on '{target}', line: '{line}'"
                )
                raise GitError(
                    errno.ENOTRECOVERABLE, f"unexpected action '{action}' on '{target}'"
                )

    return descs_modified, descs_deleted


async def _clone(repo: MaybeSecure, dest_path: Path) -> None:
    """Clones a repository from `repo` to `dest_path`."""
    try:
        _ = await run_git(["clone", "--quiet", repo, dest_path.resolve().as_posix()])
    except GitError as e:
        logger.exception(f"unable to clone '{repo}' to '{dest_path}'")
        raise GitError(
            errno.ENOTRECOVERABLE, f"unable to clone '{repo}' to '{dest_path}'"
        ) from e


async def _update(repo: MaybeSecure, repo_path: Path) -> None:
    """Update a git repository in `repo_path` from its upstream at `repo`."""
    try:
        _ = await run_git(["remote", "set-url", "origin", repo], path=repo_path)
        _ = await run_git(["remote", "update"], path=repo_path)
    except GitError as e:
        msg = f"unable to update '{repo_path}': {e}'"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e


async def _clean(repo_path: Path) -> None:
    """Clean up the git repository in `repo_path`, including submodules."""
    try:
        _ = await run_git(["reset", "--hard"], path=repo_path)
        _ = await run_git(["submodule", "foreach", "git clean -fdx"], path=repo_path)
        _ = await run_git(["clean", "-fdx"], path=repo_path)
    except GitError as e:
        msg = f"unable to clean '{repo_path}': {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e


async def git_checkout(ref: str, repo_path: Path) -> None:
    """Checkout a reference pointed to by `ref`, in repository `repo_path`."""
    try:
        _ = await run_git(["checkout", "--quiet", ref], path=repo_path)
    except GitError as e:
        msg = f"unable to checkout ref '{ref}' in repository '{repo_path}': {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e


async def git_fetch(
    remote: str, from_ref: str, to_branch: str, *, repo_path: Path | None = None
) -> None:
    """
    Fetch a reference from a remote to a new branch.

    Fetches the reference pointed to by `from_ref` from remote `remote` to a new branch
    `to_branch`. If `repo_path` is specified, run the command in said path; otherwise,
    run in current directory.
    """
    logger.debug(f"fetch from '{remote}', source: {from_ref}, dest: {to_branch}")
    try:
        _ = await run_git(["fetch", remote, f"{from_ref}:{to_branch}"], path=repo_path)
    except GitError as e:
        msg = f"unable to fetch '{from_ref}' from '{remote}' to '{to_branch}': {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e


async def git_pull(
    remote: MaybeSecure,
    *,
    from_branch: str | None = None,
    to_branch: str | None = None,
    repo_path: Path | None = None,
) -> None:
    """Pull commits from `remote`."""
    logger.debug(f"Pull from '{remote}' (from: {from_branch}, to: {to_branch})")
    try:
        cmd: CmdArgs = ["pull", remote]
        branches: str | None = None
        if from_branch:
            branches = from_branch
            if to_branch:
                branches = f"{branches}:{to_branch}"
        if branches:
            cmd.append(branches)
        _ = await run_git(cmd, path=repo_path)
    except GitError as e:
        msg = f"unable to pull from '{remote}': {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e


async def git_cherry_pick(
    sha: str, *, sha_end: str | None = None, repo_path: Path | None = None
) -> None:
    """
    Cherry-picks a given SHA to the currently checked out branch.

    If `sha_end` is provided, will cherry-pick the patches `[sha~1, sha_end]`.
    If `repo_path` is provided, run the command in said repository; otherwise, run
    in the current directory.
    """
    commit_to_pick = sha if not sha_end else f"{sha}~1..{sha_end}"
    logger.debug(f"cherry-pick commit '{commit_to_pick}'")
    try:
        _ = await run_git(["cherry-pick", "-x", commit_to_pick], path=repo_path)
    except GitError as e:
        msg = f"unable to cherry-pick '{commit_to_pick}': {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e


async def git_clone(
    repo: MaybeSecure,
    dest: Path,
    name: str,
    *,
    ref: str | None = None,
    update_if_exists: bool = False,
    clean_if_exists: bool = False,
) -> Path:
    """
    Clone a git repository, if it doesn't currently exist.

    Clone a git repository from `repo` to `dest`, using `name` for the repository, if
    it doesn't currently exist.
    If a `ref` is provided, checkout said reference.
    If `update_if_exists` is True, update the repository if it already exists.
    If `clean_if_exists` is True, clean up the existing repository.

    Returns the path to the repository.
    """
    if not dest.exists():
        logger.error(f"destination path at '{dest}' does not exist")
        raise GitError(errno.ENOENT, f"path at '{dest}' does not exist")

    repo_path = dest.resolve().joinpath(f"{name}.git")

    if repo_path.exists() and not update_if_exists:
        logger.error(f"destination repo path at '{repo_path}' exists")
        raise GitError(errno.EEXIST, f"path at '{repo_path}' already exists")

    elif repo_path.exists() and update_if_exists:
        logger.info(f"destination repo at '{repo_path}' exists, update instead")

        try:
            await _update(repo, repo_path)

            if clean_if_exists:
                await _clean(repo_path)

        except GitError as e:
            msg = f"unable to update '{repo}' at '{repo_path}': {e}"
            logger.exception(msg)
            raise GitError(errno.ENOTRECOVERABLE, msg) from e

    else:
        logger.info(f"cloning '{repo}' to new destination '{repo_path}'")
        # propagate exception to caller
        await _clone(repo, repo_path)

    if ref is not None:
        try:
            await git_checkout(ref, repo_path)

            cur_branch = await git_get_current_branch(repo_path)
            if cur_branch == ref:
                # must pull in new updates
                logger.info(f"pull in updates for branch '{ref}'")
                await git_pull(
                    repo, from_branch=ref, to_branch=ref, repo_path=repo_path
                )
        except GitError as e:
            msg = f"error cloning repository: {e}"
            logger.exception(msg)
            raise GitError(errno.ENOTRECOVERABLE, msg) from e

    return repo_path


async def git_apply(repo_path: Path, patch_path: Path) -> None:
    """Apply a patch onto the repository specified by `repo_path`."""
    try:
        _ = await run_git(["apply", patch_path.resolve().as_posix()], path=repo_path)
    except GitError as e:
        msg = f"error applying patch '{patch_path}' to '{repo_path}': {e}"
        logger.exception(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg) from e
    pass


async def git_get_sha1(repo_path: Path) -> str:
    """For the repository in `repo_path`, obtain its currently checked out SHA1."""
    val = await run_git(["rev-parse", "HEAD"], path=repo_path)
    if len(val) == 0:
        msg = f"unable to obtain current SHA1 on repository '{repo_path}"
        logger.error(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg)

    return val.strip()


async def git_get_current_branch(repo_path: Path) -> str:
    """Obtain the name of the currently checked out branch."""
    val = await run_git(["rev-parse", "--abbrev-ref", "HEAD"], path=repo_path)
    if not val:
        msg = (
            "unable to obtain current checked out branch's "
            + f"name on repository '{repo_path}'"
        )
        logger.error(msg)
        raise GitError(errno.ENOTRECOVERABLE, msg)

    return val.strip()
