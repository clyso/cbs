# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH

import logging
import logging.config
from typing import Any

from rich.console import Console


def setup_logging(*, verbose: bool, debug: bool, console: Console) -> None:
    # 1. Determine the target log level
    # Debug takes precedence if both happen to be true
    if debug:
        level = "DEBUG"
    elif verbose:
        level = "INFO"
    else:
        level = "ERROR"

    # 2. Define the configuration
    logging_config: dict[str, Any] = {  # pyright: ignore[reportExplicitAny]
        "version": 1,
        "disable_existing_loggers": False,  # Always False so you don't break third-party logs
        "formatters": {
            # RichHandler looks best with a minimal format since it adds its own timestamps/levels
            "rich_format": {
                "format": "%(message)s",
                "datefmt": "[%X]",
            }
        },
        "handlers": {
            "rich_console": {
                # dictConfig will automatically instantiate this class for you
                "class": "rich.logging.RichHandler",
                "formatter": "rich_format",
                # You can pass constructor kwargs directly into the dictionary!
                "rich_tracebacks": True,
                "console": console,
            }
        },
        "loggers": {
            # Configure the root app project
            "crt": {
                "handlers": ["rich_console"],
                "level": level,
                "propagate": False,
            },
            # Configure the extracted library project simultaneously
            "cbscommon": {
                "handlers": ["rich_console"],
                "level": level,
                "propagate": False,
            },
        },
    }

    # 3. Apply the configuration instantly
    logging.config.dictConfig(logging_config)
