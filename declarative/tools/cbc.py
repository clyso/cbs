# CBC - CES Build Service Client
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

# pyright: reportAny=false

import logging
import sys
from copy import copy
from functools import update_wrapper, wraps
from pathlib import Path
from typing import Callable, Concatenate, ParamSpec, TypeVar, override

import click
import httpx
import pydantic
from cbslib.auth.users import User
from cbslib.config.user import CBSUserConfig
from ceslib.errors import CESError
from ceslib.logger import log as parent_logger

_DEFAULT_CONFIG_PATH = Path.cwd().joinpath("cbc-config.json")


log = parent_logger.getChild("cbc")


class CBCError(CESError):
    @override
    def __str__(self) -> str:
        return "CBC Error" + (f": {self.msg}" if self.msg else "")


class CBCConnectionError(CBCError):
    @override
    def __str__(self) -> str:
        return "Connection Error" + (f": {self.msg}" if self.msg else "")


class Ctx:
    config: CBSUserConfig | None
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
            log.debug(f"no parent context found for '{f.__name__}'")
        else:
            new_ctx = copy(parent_ctx)
            if not parent_ctx.logger:
                new_ctx.logger = log.getChild(f.__name__)
            else:
                new_ctx.logger = parent_ctx.logger.getChild(f.__name__)
            curr_ctx.obj = new_ctx
        return f(*args, **kwargs)

    return update_wrapper(inner, f)


def pass_config(f: Callable[Concatenate[CBSUserConfig, P], R]) -> Callable[P, R]:
    """Pass the user's config to the function. If not set, exit with error."""

    def inner(*args: P.args, **kwargs: P.kwargs) -> R:
        curr_ctx = click.get_current_context()
        ctx = curr_ctx.find_object(Ctx)
        if not ctx:
            log.error(f"missing context for '{f.__name__}'")
            sys.exit(1)
        config = ctx.config
        if not config:
            log.error(f"missing config for '{f.__name__}'")
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
            log.error(f"missing context for '{f.__name__}'")
            sys.exit(1)
        logger = ctx.logger
        if not logger:
            log.error(f"missing logger for '{f.__name__}'")
            sys.exit(1)
        return f(logger, *args, **kwargs)
        # return curr_ctx.invoke(f, config, *args, **kwargs)

    return update_wrapper(inner, f)


class CBCClient:
    _client: httpx.Client
    _logger: logging.Logger

    def __init__(
        self,
        logger: logging.Logger,
        base_url: str,
        *,
        token: str | None = None,
        verify: bool = False,
    ) -> None:
        self._logger = logger

        headers = None if not token else {"Authorization": f"Bearer {token}"}

        self._client = httpx.Client(
            base_url=f"{base_url}/api",
            headers=headers,
            verify=verify,
        )

    def get(
        self, ep: str, *, params: httpx.QueryParams | None = None
    ) -> httpx.Response:
        try:
            return self._client.get(ep, params=params)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg)
        except Exception as e:
            msg = f"error getting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg)


_WithEPFn = Callable[Concatenate[logging.Logger, CBCClient, str, P], R]
_WithClientFn = Callable[Concatenate[logging.Logger, CBSUserConfig, P], R]
_EPFnWrapper = Callable[[_WithEPFn[P, R]], _WithClientFn[P, R]]


def endpoint(ep: str, *, verify: bool = False) -> _EPFnWrapper[P, R]:
    ep = ep.lstrip("/")

    def inner(
        fn: _WithEPFn[P, R],
    ) -> _WithClientFn[P, R]:
        @wraps(fn)
        def wrapper(
            logger: logging.Logger,
            cfg: CBSUserConfig,
            *args: P.args,
            **kwargs: P.kwargs,
        ) -> R:
            host = cfg.host.rstrip("/")
            client = CBCClient(
                logger, host, token=cfg.login_info.token.decode("utf-8"), verify=verify
            )
            return fn(logger, client, ep, *args, **kwargs)

        return wrapper

    return inner


def _auth_ping(logger: logging.Logger, host: str, verify: bool = False) -> bool:
    """Ping the build service server."""
    try:
        host = host.rstrip("/")
        client = CBCClient(logger, host, verify=verify)
        _ = client.get("/auth/ping")
        return True
    except CBCConnectionError as e:
        logger.error(f"unable to connect to server: {e}")
        return False
    except CBCError as e:
        logger.error(f"unexpected error pinging server: {e}")
        return False


@endpoint("/auth/whoami")
def _auth_whoami(logger: logging.Logger, client: CBCClient, ep: str) -> tuple[str, str]:
    try:
        r = client.get(ep)
        res = r.read()
        logger.debug(f"whoami: {res}")
    except CBCError as e:
        logger.error(f"unable to obtain whoami: {e}")
        raise e

    try:
        user = User.model_validate_json(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.error(msg)
        raise CBCError(msg)

    return (user.email, user.name)


_cbc_help_message = """CES Build Service Client

Interacts with a CES Build Service, allowing the user to perform various
build-related actions, such as listing existing builds, on-going builds,
and trigger new builds.

See subcommands' descriptions for more information.
"""


@click.group(help=_cbc_help_message)
@click.option(
    "-d",
    "--debug",
    is_flag=True,
    default=False,
    help="Enable debug logging",
)
@click.option(
    "-c",
    "--config",
    "config_path",
    type=click.Path(
        exists=True,
        file_okay=True,
        dir_okay=False,
        readable=True,
        resolve_path=True,
        path_type=Path,
    ),
    required=False,
    help="Specify cbs config JSON file",
)
@pass_ctx
def main(ctx: Ctx, debug: bool, config_path: Path | None) -> None:
    if debug:
        parent_logger.setLevel(logging.DEBUG)

    logging.getLogger("httpx").setLevel(logging.DEBUG if debug else logging.CRITICAL)

    log.info(f"config path: {config_path}")
    user_config_path: Path = _DEFAULT_CONFIG_PATH
    if config_path:
        user_config_path = config_path

    if user_config_path.exists() and user_config_path.is_file():
        ctx.config = CBSUserConfig.load(user_config_path)


@main.group(help="auth related commands")
@update_ctx
def auth() -> None:
    pass


@auth.command(help="Log into a CES Build Service instance")
@click.argument("host", type=str, metavar="URL", required=True)
@update_ctx
@pass_logger
def login(logger: logging.Logger, host: str) -> None:
    logger.debug(f"login to {host}")
    if not _auth_ping(logger, host):
        click.echo(f"server at '{host}' not reachable")
        sys.exit(1)

    click.echo("please follow the URL to login")
    click.echo()
    click.echo(f"\t{host}/api/auth/login")
    click.echo()
    click.echo(f"Once logged in, copy the file to {_DEFAULT_CONFIG_PATH}")
    pass


@auth.command(help="Checks user is logged in")
@update_ctx
@pass_logger
@pass_config
def whoami(config: CBSUserConfig, logger: logging.Logger) -> None:
    logger.debug(f"config: {config}")
    try:
        email, name = _auth_whoami(logger, config)
        click.echo(f"email: {email}")
        click.echo(f" name: {name}")
    except Exception as e:
        click.echo(f"error obtaining whoami: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
