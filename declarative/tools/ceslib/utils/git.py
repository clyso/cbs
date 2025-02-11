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
from pathlib import Path
import re
import shlex
import subprocess
from typing import override

from ceslib.errors import CESError
from ceslib.utils import log as parent_logger

log = parent_logger.getChild("git")


class GitError(CESError):
    retcode: int
    msg: str

    def __init__(self, retcode: int, msg: str) -> None:
        super().__init__()
        self.retcode = retcode
        self.msg = msg

    @override
    def __str__(self) -> str:
        return f"git error: {self.msg} (retcode: {self.retcode})"


class GitConfigNotSetError(GitError):
    def __init__(self, what: str) -> None:
        super().__init__(errno.ENOENT, f"{what} not set in config")


def run_git(args: str) -> str:
    cmd = shlex.split(args)
    log.debug(f"run {cmd}")
    p = subprocess.run(["git"] + cmd, capture_output=True)
    if p.returncode != 0:
        log.error(f"unable to obtain result from git '{args}': {p.stderr}")
        raise GitError(p.returncode, p.stderr.decode("utf-8"))

    return p.stdout.decode("utf-8")


def get_git_user() -> tuple[str, str]:
    def _run_git_config_for(v: str) -> str:
        val = run_git(f"config {v}")
        if len(val) == 0:
            log.error(f"'{v}' not set in git config")
            raise GitConfigNotSetError(v)

        return val.strip()

    user_name = _run_git_config_for("user.name")
    user_email = _run_git_config_for("user.email")
    assert len(user_name) > 0 and len(user_email) > 0
    return (user_name, user_email)


def get_git_repo_root() -> Path:
    val = run_git("rev-parse --show-toplevel")
    if len(val) == 0:
        log.error("unable to obtain toplevel git directory path")
        raise GitError(errno.ENOENT, "top-level git directory not found")

    return Path(val.strip())


def get_git_modified_paths(
    base_sha: str, ref: str, repo_path: str | None = None
) -> tuple[list[Path], list[Path]]:
    try:
        val = run_git(
            "diff-tree --diff-filter=ACDMR --ignore-all-space "
            + f"--no-commit-id --name-status -r {base_sha} {ref}"
            + (f" -- {repo_path}" if repo_path is not None else "")
        )
    except GitError as e:
        log.error(f"error: unable to obtain latest patch: {e}")
        raise GitError(
            errno.ENOTRECOVERABLE,
            f"unable to obtain patches between {base_sha} and {ref}",
        )

    if len(val) == 0:
        log.debug(f"no relevant patches found between {base_sha} and {ref}")
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
            log.debug(f"'{line}' does not match")
            continue

        action = m.group(1)
        target = m.group(2)
        log.debug(f"action: {action}, target: {target}")

        match action:
            case "D":
                descs_deleted.append(Path(target))
            case "A" | "C" | "M" | "R":
                descs_modified.append(Path(target))
            case _:
                log.error(f"unexpected action '{action}' on '{target}', line: '{line}'")
                raise GitError(
                    errno.ENOTRECOVERABLE, f"unexpected action '{action}' on '{target}'"
                )

    return descs_modified, descs_deleted
