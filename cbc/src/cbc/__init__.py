# cbc - CBS service client
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

import logging
from pathlib import Path
from typing import override

from cbscore.errors import CESError
from cbscore.logger import set_debug_logging as cbscore_set_debug_logging
from cbsdcore.logger import set_debug_logging as cbsdcore_set_debug_logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("cbc")


class CBCError(CESError):
    @override
    def __str__(self) -> str:
        return "CBC Error" + (f": {self.msg}" if self.msg else "")


def set_debug_logging() -> None:
    """Set debug logging for CBC."""
    logger.setLevel(logging.DEBUG)
    cbscore_set_debug_logging()
    cbsdcore_set_debug_logging()


CBC_DEFAULT_CONFIG_PATH = Path.cwd() / "cbc-config.json"
