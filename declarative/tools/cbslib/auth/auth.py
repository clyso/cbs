# CBS - auth library - tokens
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

from datetime import datetime as dt
from datetime import timedelta as td
from datetime import timezone as tz
from typing import Annotated, cast

import pydantic
import pydantic_core
import pyseto
from cbslib.auth import log as parent_logger
from cbslib.config.server import get_config
from fastapi import Depends, HTTPException, status
from fastapi.security import OAuth2AuthorizationCodeBearer

log = parent_logger.getChild("auth")


class CBSTokenInfo(pydantic.BaseModel):
    user: str
    expires: dt | None


class CBSToken(pydantic.BaseModel):
    token: bytes
    info: CBSTokenInfo


def token_create(user: str) -> CBSToken:
    """Creates a new CBSToken, including its paseto token, for the given user."""
    config = get_config()
    expiration = (
        None
        if not config.secrets.jwt_ttl_minutes
        else dt.now(tz.utc) + td(minutes=config.secrets.jwt_ttl_minutes)
    )
    info = CBSTokenInfo(user=user, expires=expiration)
    info_payload = pydantic_core.to_jsonable_python(info)  # pyright: ignore[reportAny]

    key = pyseto.Key.new(version=4, purpose="local", key=config.secrets.jwt_secret)
    token = pyseto.encode(  # pyright: ignore[reportUnknownMemberType]
        key,
        payload=info_payload,  # pyright: ignore[reportAny]
    )
    return CBSToken(token=token, info=info)


_oauth_scheme = OAuth2AuthorizationCodeBearer("/api/auth/login", "/api/auth/login")
_AuthToken = Annotated[str, Depends(_oauth_scheme)]


def token_decode(token: _AuthToken) -> CBSTokenInfo:
    print(f"token_decode, token: {token}")

    config = get_config()
    key = pyseto.Key.new(version=4, purpose="local", key=config.secrets.jwt_secret)
    try:
        decoded_token = pyseto.decode(key, token)
    except Exception as e:
        log.error(f"error decoding token: {e}")
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, detail="Invalid user token")

    try:
        return CBSTokenInfo.model_validate_json(cast(bytes, decoded_token.payload))
    except pydantic.ValidationError:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, detail="Invalid user token")


AuthTokenInfo = Annotated[CBSTokenInfo, Depends(token_decode)]
