# CBS service library - routes - auth utilities
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

from typing import Annotated

from cbsdcore.api.responses import BaseErrorModel
from cbsdcore.auth.token import TokenInfo
from fastapi import Depends, HTTPException, status
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from cbslib.auth.auth import UnauthorizedTokenError, token_decode

_http_bearer = HTTPBearer()


responses_auth_token = {
    401: {
        "model": BaseErrorModel,
        "description": "User not authorized to perform request",
    }
}


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


def get_user_token(token: _AuthToken) -> TokenInfo:
    try:
        return token_decode(token)
    except UnauthorizedTokenError as e:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED, detail=e.msg
        ) from None


AuthTokenInfo = Annotated[TokenInfo, Depends(get_user_token)]
