# CES library - utilities
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

import subprocess

from ceslib.errors import CESError
from ceslib.logging import log as root_logger

log = root_logger.getChild("utils")


def run_cmd(cmd: list[str], env: dict[str, str] | None = None) -> tuple[int, str, str]:
    try:
        p = subprocess.run(cmd, env=env, capture_output=True)
    except OSError as e:
        log.error(f"error running '{cmd}': {e}")
        raise CESError()

    if p.returncode != 0:
        log.error(f"error running '{cmd}': retcode = {p.returncode}, res: {p.stderr}")
        return (p.returncode, "", p.stderr.decode("utf-8"))

    return (0, p.stdout.decode("utf-8"), p.stderr.decode("utf-8"))
