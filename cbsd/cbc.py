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
from collections.abc import Callable
from copy import copy
from functools import update_wrapper, wraps
from pathlib import Path
from typing import Any, Concatenate, ParamSpec, TypeVar, override

import click
import httpx
import pydantic
from httpx import _types as httpx_types  # pyright: ignore[reportPrivateUsage]

from cbscore.errors import CESError
from cbscore.logger import logger as parent_logger
from cbscore.versions.create import create
from cbscore.versions.desc import VersionDescriptor
from cbscore.versions.errors import VersionError
from cbsdcore.api.responses import BaseErrorModel, NewBuildResponse
from cbsdcore.auth.user import User, UserConfig
from cbsdcore.builds.types import BuildEntry

_DEFAULT_CONFIG_PATH = Path.cwd().joinpath("cbc-config.json")


logger = parent_logger.getChild("cbc")


class CBCError(CESError):
    @override
    def __str__(self) -> str:
        return "CBC Error" + (f": {self.msg}" if self.msg else "")


class CBCConnectionError(CBCError):
    @override
    def __str__(self) -> str:
        return "Connection Error" + (f": {self.msg}" if self.msg else "")


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

    def _maybe_handle_error(self, res: httpx.Response) -> None:
        if res.is_error:
            try:
                err = BaseErrorModel.model_validate(res.json())
                msg = err.detail
            except pydantic.ValidationError:
                msg = res.read().decode("utf-8")

            raise CBCError(msg)

    def get(
        self, ep: str, *, params: httpx_types.QueryParamTypes | None = None
    ) -> httpx.Response:
        try:
            res = self._client.get(ep, params=params)
            self._maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.exception(msg)
            raise CBCConnectionError(msg) from e
        except Exception as e:
            msg = f"error getting '{ep}': {e}"
            self._logger.exception(msg)
            raise CBCError(msg) from e
        return res

    def post(
        self,
        ep: str,
        data: Any,  # pyright: ignore[reportExplicitAny]
    ) -> httpx.Response:
        try:
            res = self._client.post(ep, json=data)
            self._maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.exception(msg)
            raise CBCConnectionError(msg) from e
        except Exception as e:
            msg = f"error posting '{ep}': {e}"
            self._logger.exception(msg)
            raise CBCError(msg) from e
        return res

    def delete(
        self, ep: str, params: httpx_types.QueryParamTypes | None = None
    ) -> httpx.Response:
        try:
            res = self._client.delete(ep, params=params)
            self._maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.exception(msg)
            raise CBCConnectionError(msg) from e
        except Exception as e:
            msg = f"error deleting '{ep}': {e}"
            self._logger.exception(msg)
            raise CBCError(msg) from e
        return res


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
    except CBCConnectionError:
        logger.exception("unable to connect to server")
        return False
    except CBCError:
        logger.exception("unexpected error pinging server")
        return False
    return True


@endpoint("/auth/whoami")
def _auth_whoami(logger: logging.Logger, client: CBCClient, ep: str) -> tuple[str, str]:
    try:
        r = client.get(ep)
        res = r.read()
        logger.debug(f"whoami: {res}")
    except CBCError as e:
        logger.exception("unable to obtain whoami")
        raise e  # noqa: TRY201

    try:
        user = User.model_validate_json(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.exception(msg)
        raise CBCError(msg) from None

    return (user.email, user.name)


@endpoint("/builds/new")
def _build_new(
    logger: logging.Logger, client: CBCClient, ep: str, desc: VersionDescriptor
) -> NewBuildResponse:
    data = desc.model_dump(mode="json")
    try:
        r = client.post(ep, data)
        res = r.json()
        logger.debug(f"new build: {res}")
    except CBCError as e:
        logger.exception("unable to create new build")
        raise e  # noqa: TRY201

    try:
        return NewBuildResponse.model_validate(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.exception(msg)
        raise CBCError(msg) from None


@endpoint("/builds/status")
def _build_list(
    logger: logging.Logger, client: CBCClient, ep: str, all: bool
) -> list[BuildEntry]:
    try:
        r = client.get(ep, params={"all": all})
        res = r.json()
    except CBCError as e:
        logger.exception("unable to list builds")
        raise e  # noqa: TRY201

    ta = pydantic.TypeAdapter(list[BuildEntry])
    try:
        return ta.validate_python(res)
    except pydantic.ValidationError:
        msg = f"error validating server result: {res}"
        logger.exception(msg)
        raise CBCError(msg) from None


@endpoint("/builds/abort")
def _build_abort(
    logger: logging.Logger, client: CBCClient, ep: str, build_id: str, force: bool
) -> None:
    try:
        params = {"force": force} if force else None
        _ = client.delete(f"{ep}/{build_id}", params=params)
    except CBCError as e:
        logger.exception(f"unable to abort build '{build_id}'")
        raise e  # noqa: TRY201


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

    logger.info(f"config path: {config_path}")
    user_config_path: Path = _DEFAULT_CONFIG_PATH
    if config_path:
        user_config_path = config_path

    if user_config_path.exists() and user_config_path.is_file():
        ctx.config = UserConfig.load(user_config_path)


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
def whoami(config: UserConfig, logger: logging.Logger) -> None:
    logger.debug(f"config: {config}")
    try:
        email, name = _auth_whoami(logger, config)
        click.echo(f"email: {email}")
        click.echo(f" name: {name}")
    except Exception as e:
        click.echo(f"error obtaining whoami: {e}")
        sys.exit(1)


@main.group(help="build related commands")
@update_ctx
def build() -> None:
    pass


@build.command("new", help="Create new build")
@click.argument("version", type=str, metavar="VERSION", required=True)
@click.option(
    "-c",
    "--component",
    "components",
    type=str,
    multiple=True,
    required=True,
    metavar="NAME@VERSION",
    help="Component's version (e.g., 'ceph@abcde1234')",
)
@click.option(
    "--override-component",
    "component_overrides",
    type=str,
    multiple=True,
    required=False,
    metavar="COMPONENT=URL",
    help="Override component's location",
)
@click.option(
    "--distro",
    type=str,
    required=False,
    default="rockylinux:9",
    metavar="NAME",
    help="Distribution to use for this release",
)
@click.option(
    "--el-version",
    type=int,
    required=False,
    default=9,
    metavar="VERSION",
    help="Distribution's EL version",
)
@click.option(
    "--registry",
    type=str,
    required=False,
    default="harbor.clyso.com",
    metavar="URL",
    help="Registry for this release's image",
)
@click.option(
    "--image-name",
    type=str,
    required=False,
    default="ces/ceph/ceph",
    metavar="NAME",
    help="Name for this release's image",
)
@click.option(
    "--image-tag",
    type=str,
    required=False,
    metavar="TAG",
    help="Tag for this release's image",
)
@update_ctx
@pass_logger
@pass_config
def new_build(
    config: UserConfig,
    logger: logging.Logger,
    version: str,
    components: list[str],
    component_overrides: list[str],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
) -> None:
    try:
        email, name = _auth_whoami(logger, config)
    except Exception as e:
        click.echo(f"error obtaining user's info: {e}", err=True)
        sys.exit(1)

    try:
        version_type, desc = create(
            version,
            version_types,
            components,
            component_overrides,
            distro,
            el_version,
            registry,
            image_name,
            image_tag,
            name,
            email,
        )
    except (VersionError, Exception) as e:
        click.echo(f"error creating version descriptor: {e}")
        sys.exit(1)

    try:
        res = _build_new(logger, config, desc)
    except CBCError as e:
        click.echo(f"error triggering build: {e}")
        sys.exit(1)

    click.echo(f"version type: {version_type.name}")
    click.echo(f"     task id: {res.task_id}")
    click.echo(f"       state: {res.state}")


@build.command("list", help="List builds from the build service")
@click.option("--all", is_flag=True, default=False, help="List all known builds")
@update_ctx
@pass_logger
@pass_config
def build_list(config: UserConfig, logger: logging.Logger, all: bool) -> None:
    try:
        lst = _build_list(logger, config, all)
    except CBCError as e:
        click.echo(f"error obtaining build list: {e}", err=True)
        sys.exit(1)

    if not lst:
        click.echo("no builds found")
        return

    for entry in lst:
        click.echo("---")
        click.echo(f" build id: {entry.task_id}")
        click.echo(f"     user: {entry.user}")
        click.echo(f"    state: {entry.state}")
        click.echo(f"submitted: {entry.submitted}")
        click.echo(f" finished: {entry.finished}")

    pass


@build.command("abort", help="Abort an existing build")
@click.argument("build_id", type=str, required=True, metavar="ID")
@click.option(
    "--force",
    is_flag=True,
    required=False,
    default=False,
    help="Force aborting build, regardless of whom has created it",
)
@update_ctx
@pass_logger
@pass_config
def build_abort(
    config: UserConfig, logger: logging.Logger, build_id: str, force: bool
) -> None:
    try:
        _build_abort(logger, config, build_id, force)
    except CBCError as e:
        click.echo(f"error aborting build '{build_id}': {e}", err=True)
        sys.exit(1)

    click.echo(f"successfully aborted build '{build_id}'")


if __name__ == "__main__":
    main()
