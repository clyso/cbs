# CES library - CES containers
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

from pathlib import Path
from typing import override

from ceslib.errors import CESError
from ceslib.logger import log as root_logger

log = root_logger.getChild("containers")


class ContainerError(CESError):
    @override
    def __str__(self) -> str:
        return f"Container Error: {self.msg}"


def find_path_relative_to(name: str, hint: Path, root: Path) -> Path | None:
    p = hint
    while True:
        candidate = p.joinpath(name)
        if candidate.exists():
            return candidate

        if p == root:
            break
        p = p.parent
    return None
