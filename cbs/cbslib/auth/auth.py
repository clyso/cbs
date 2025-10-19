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
from datetime import datetime as dt
from datetime import timedelta as td
from typing import Annotated, cast

import pydantic
import pydantic_core
import pyseto
from fastapi import Depends, HTTPException, status
from fastapi.security import (
    HTTPAuthorizationCredentials,
    HTTPBearer,
)

from cbslib.auth import logger as parent_logger
from cbslib.config import get_config

logger = parent_logger.getChild("auth")


class CBSTokenInfo(pydantic.BaseModel):
    user: str
    expires: dt | None


class CBSToken(pydantic.BaseModel):
    token: bytes
    info: CBSTokenInfo


def token_create(user: str) -> CBSToken:
    """Create a new CBSToken, including its paseto token, for the given user."""
    config = get_config()
    assert config.server, "unexpected server config missing"
    expiration = (
        None
        if not config.server.secrets.token_secret_ttl_minutes
        else dt.now(datetime.UTC)
        + td(minutes=config.server.secrets.token_secret_ttl_minutes)
    )
    info = CBSTokenInfo(user=user, expires=expiration)
    info_payload = pydantic_core.to_jsonable_python(info)  # pyright: ignore[reportAny]

    key = pyseto.Key.new(
        version=4, purpose="local", key=config.server.secrets.token_secret_key
    )
    token = pyseto.encode(  # pyright: ignore[reportUnknownMemberType]
        key,
        payload=info_payload,  # pyright: ignore[reportAny]
    )
    return CBSToken(token=token, info=info)


_http_bearer = HTTPBearer()


def _token_auth(
    authorization: Annotated[HTTPAuthorizationCredentials, Depends(_http_bearer)],
) -> str:
    failed_error = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Invalid authorization",
        headers={"WWW-Authorization": "Bearer"},
    )

    if authorization.scheme.lower() != "bearer" or not authorization.credentials:
        raise failed_error

    return authorization.credentials


_AuthToken = Annotated[str, Depends(_token_auth)]


def token_decode(token: _AuthToken) -> CBSTokenInfo:
    print(f"token_decode, token: {token}")

    config = get_config()
    assert config.server, "unexpected server config missing"
    key = pyseto.Key.new(
        version=4, purpose="local", key=config.server.secrets.token_secret_key
    )
    try:
        decoded_token = pyseto.decode(key, token)
    except Exception as e:
        logger.exception("error decoding token")
        raise HTTPException(
            status.HTTP_401_UNAUTHORIZED, detail="Invalid user token"
        ) from e

    try:
        return CBSTokenInfo.model_validate_json(cast(bytes, decoded_token.payload))
    except pydantic.ValidationError:
        raise HTTPException(
            status.HTTP_401_UNAUTHORIZED, detail="Invalid user token"
        ) from None


AuthTokenInfo = Annotated[CBSTokenInfo, Depends(token_decode)]
