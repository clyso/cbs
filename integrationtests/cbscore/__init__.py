# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.


import shutil
import subprocess
from pathlib import Path


def git(repo_path: Path, *args: str):
    _ = _execute("git", "-C", str(repo_path), *args)


def podman(*args: str) -> str:
    return _execute("podman", *args)


def _execute(executable: str, *args: str) -> str:
    absolute_path = shutil.which(executable)
    if not absolute_path:
        raise RuntimeError(f"{executable} not found in PATH")
    result = subprocess.run(  # noqa: S603 - Arguments are controlled by the internal build logic
        [absolute_path, *args], check=True, capture_output=True, text=True
    )
    return result.stdout
