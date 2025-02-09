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
import shlex
import subprocess
import sys

from ceslib.utils import log as parent_logger

log = parent_logger.getChild("git")


def run_git(args: str) -> str:
    cmd = shlex.split(args)
    log.debug(f"run {cmd}")
    p = subprocess.run(["git"] + cmd, capture_output=True, stderr=None)
    if p.returncode != 0:
        log.error(f"unable to obtain result from git '{args}'")
        sys.exit(p.returncode)

    return p.stdout.decode("utf-8")


def get_git_user() -> tuple[str, str]:
    def _run_git_config_for(v: str) -> str:
        val = run_git(f"config {v}")
        if len(val) == 0:
            log.error(f"'{v}' not set in git config")
            sys.exit(errno.EINVAL)

        return val.strip()

    user_name = _run_git_config_for("user.name")
    user_email = _run_git_config_for("user.email")
    assert len(user_name) > 0 and len(user_email) > 0
    return (user_name, user_email)


def get_git_repo_root() -> Path:
    val = run_git("rev-parse --show-toplevel")
    if len(val) == 0:
        log.error("unable to obtain toplevel git directory path")
        sys.exit(errno.ENOENT)

    return Path(val.strip())
