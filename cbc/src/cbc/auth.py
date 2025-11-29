# cbc - auth
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

import pydantic

from cbc import CBCError
from cbc import logger as parent_logger
from cbc.client import CBCClient, CBCConnectionError
from cbc.cmds import endpoint
from cbsdcore.auth.user import User

logger = parent_logger.getChild("auth")


def auth_ping(logger: logging.Logger, host: str, verify: bool = False) -> bool:
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
def auth_whoami(logger: logging.Logger, client: CBCClient, ep: str) -> tuple[str, str]:
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
