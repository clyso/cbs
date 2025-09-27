# CES library - CES builder
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

from typing import override

from ceslib.errors import CESError
from ceslib.logger import logger as root_logger

logger = root_logger.getChild("builder")


class BuilderError(CESError):
    @override
    def __str__(self) -> str:
        return f"Builder Error: {self.msg}"


class MissingScriptError(BuilderError):
    """Represents a missing script, required for execution."""

    script: str

    def __init__(self, script: str, *, msg: str | None = None) -> None:
        super().__init__(msg)
        self.script = script

    @override
    def __str__(self) -> str:
        return f"Missing script '{self.script}'" + f": {self.msg}" if self.msg else ""
