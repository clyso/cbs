# cbc - commands
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
import sys
from collections.abc import Callable
from copy import copy
from functools import update_wrapper, wraps
from typing import Concatenate, ParamSpec, TypeVar

import click
from cbsdcore.auth.user import UserConfig

from cbc import logger as parent_logger
from cbc.client import CBCClient

logger = parent_logger.getChild("cmds")


class Ctx:
    config: UserConfig | None
    logger: logging.Logger | None

    def __init__(self) -> None:
        self.config = None
        self.logger = None


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)

R = TypeVar("R")
T = TypeVar("T")
P = ParamSpec("P")


def update_ctx(
    f: Callable[P, R],
) -> Callable[P, R]:
    """Update current context, create sub-logger."""

    def inner(*args: P.args, **kwargs: P.kwargs) -> R:
        curr_ctx = click.get_current_context()
        parent_ctx = curr_ctx.find_object(Ctx)
        if not parent_ctx:
            logger.debug(f"no parent context found for '{f.__name__}'")
        else:
            new_ctx = copy(parent_ctx)
            if not parent_ctx.logger:
                new_ctx.logger = logger.getChild(f.__name__)
            else:
                new_ctx.logger = parent_ctx.logger.getChild(f.__name__)
            curr_ctx.obj = new_ctx
        return f(*args, **kwargs)

    return update_wrapper(inner, f)


def pass_config(f: Callable[Concatenate[UserConfig, P], R]) -> Callable[P, R]:
    """Pass the user's config to the function. If not set, exit with error."""

    def inner(*args: P.args, **kwargs: P.kwargs) -> R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            logger.error(f"missing context for '{f.__name__}'")
            sys.exit(1)
        config = ctx.config
        if not config:
            logger.error(f"missing config for '{f.__name__}'")
            sys.exit(1)
        return f(config, *args, **kwargs)
        # return curr_ctx.invoke(f, config, *args, **kwargs)

    return update_wrapper(inner, f)


def pass_logger(f: Callable[Concatenate[logging.Logger, P], R]) -> Callable[P, R]:
    """Pass a sub-logger to the function. If not set, exit with error."""

    def inner(*args: P.args, **kwargs: P.kwargs) -> R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            logger.error(f"missing context for '{f.__name__}'")
            sys.exit(1)
        our_logger = ctx.logger
        if not our_logger:
            logger.error(f"missing logger for '{f.__name__}'")
            sys.exit(1)
        return f(our_logger, *args, **kwargs)
        # return curr_ctx.invoke(f, config, *args, **kwargs)

    return update_wrapper(inner, f)


_WithEPFn = Callable[Concatenate[logging.Logger, CBCClient, str, P], R]
_WithClientFn = Callable[Concatenate[logging.Logger, UserConfig, P], R]
_EPFnWrapper = Callable[[_WithEPFn[P, R]], _WithClientFn[P, R]]


def endpoint(ep: str, *, verify: bool = False) -> _EPFnWrapper[P, R]:
    ep = ep.lstrip("/")

    def inner(
        fn: _WithEPFn[P, R],
    ) -> _WithClientFn[P, R]:
        @wraps(fn)
        def wrapper(
            logger: logging.Logger,
            cfg: UserConfig,
            *args: P.args,
            **kwargs: P.kwargs,
        ) -> R:
            host = cfg.host.rstrip("/")
            client = CBCClient(
                logger,
                host,
                token=cfg.login_info.token.get_secret_value().decode("utf-8"),
                verify=verify,
            )
            return fn(logger, client, ep, *args, **kwargs)

        return wrapper

    return inner
