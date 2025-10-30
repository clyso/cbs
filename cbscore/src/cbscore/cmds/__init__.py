# cbsbuild - commands
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
import sys
from collections.abc import Callable
from functools import update_wrapper
from pathlib import Path
from typing import Concatenate

import click

from cbscore.config import Config
from cbscore.logger import logger as root_logger

logger = root_logger.getChild("cbsbuild")


class Ctx:
    config_path: Path | None = None
    vault_config_path: Path | None = None


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)


def with_config[R, T, **P](
    f: Callable[Concatenate[Config, P], R],
) -> Callable[P, R]:
    """Pass the CES patches repo path from the context to the function."""

    def inner(*args: P.args, **kwargs: P.kwargs) -> R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            logger.error(f"missing context for '{f.__name__}'")
            sys.exit(errno.ENOTRECOVERABLE)
        if not ctx.config_path:
            logger.error("configuration file path not provided")
            sys.exit(errno.EINVAL)

        try:
            config = Config.load(ctx.config_path)
        except Exception as e:
            logger.error(f"unable to read configuration file: {e}")
            sys.exit(errno.ENOTRECOVERABLE)

        return f(config, *args, **kwargs)

    return update_wrapper(inner, f)


def set_log_level(lvl: int) -> None:
    root_logger.setLevel(lvl)
