# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH

import logging

logger = logging.getLogger("cbscommon")
logger.setLevel(logging.ERROR)


def logger_set_handler(handler: logging.Handler) -> None:
    logger.propagate = False
    logger.addHandler(handler)
