# CBS server library - auth library - tokens
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

import datetime
import logging
from datetime import datetime as dt
from datetime import timedelta as td
from typing import cast

import pydantic
import pydantic_core
import pyseto
from cbsdcore.auth.token import Token, TokenInfo

from cbslib.auth import AuthError
from cbslib.auth import logger as parent_logger
from cbslib.config.config import get_config

logger = parent_logger.getChild("auth")
logger.setLevel(logging.ERROR)


class UnauthorizedTokenError(AuthError):
    pass


def token_create(user: str) -> Token:
    """Create a new CBSToken, including its paseto token, for the given user."""
    config = get_config()
    assert config.server, "unexpected server config missing"
    expiration = (
        None
        if not config.server.secrets.token_secret_ttl_minutes
        else dt.now(datetime.UTC)
        + td(minutes=config.server.secrets.token_secret_ttl_minutes)
    )
    info = TokenInfo(user=user, expires=expiration)
    info_payload = pydantic_core.to_jsonable_python(info)  # pyright: ignore[reportAny]

    key = pyseto.Key.new(
        version=4, purpose="local", key=config.server.secrets.token_secret_key
    )
    token = pyseto.encode(  # pyright: ignore[reportUnknownMemberType]
        key,
        payload=info_payload,  # pyright: ignore[reportAny]
    )
    return Token(token=pydantic.SecretBytes(token), info=info)


def token_decode(token: str) -> TokenInfo:
    """Decode the provided token."""
    logger.debug(f"decode token: {token}")

    config = get_config()
    assert config.server, "unexpected server config missing"
    key = pyseto.Key.new(
        version=4, purpose="local", key=config.server.secrets.token_secret_key
    )
    try:
        decoded_token = pyseto.decode(key, token)
    except Exception as e:
        msg = f"error decoding provided token: {e}"
        logger.warning(msg)
        raise UnauthorizedTokenError(msg=msg) from None

    try:
        return TokenInfo.model_validate_json(cast(bytes, decoded_token.payload))
    except pydantic.ValidationError as e:
        msg = "malformed user token"
        logger.error(f"{msg}: {e}")
        raise UnauthorizedTokenError(msg=msg) from None
