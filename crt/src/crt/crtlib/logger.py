# crt - logger
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

import logging

logger = logging.getLogger("crt")
logger.setLevel(logging.ERROR)


def logger_set_handler(handler: logging.Handler) -> None:
    logger.propagate = False
    logger.addHandler(handler)


def logger_unset_handler(handler: logging.Handler) -> None:
    logger.removeHandler(handler)
    logger.propagate = True
