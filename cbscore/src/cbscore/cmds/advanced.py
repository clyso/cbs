# cbsbuild - commands - advanced commands
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


import click

from cbscore.cmds import logger as parent_logger

logger = parent_logger.getChild("advanced")


@click.group(hidden=True)
def cmd_advanced() -> None:
    pass
