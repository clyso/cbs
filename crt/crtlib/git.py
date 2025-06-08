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


class GitPatchDiffError(CRTError):
    @override
    def __str__(self) -> str:
        return "patches error" + (f": {self.msg}" if self.msg else "")


class GitEmptyPatchDiffError(GitPatchDiffError):
    pass


def git_check_patches_diff(
    ceph_git_path: Path,
    upstream_ref: str,
    head_ref: str,
    *,
    limit: str | None = None,
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
