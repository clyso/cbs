# CBS server library - auth library - OAuth
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

# pyright: reportMissingTypeStubs=false
# pyright: reportUnknownVariableType=false
# pyright: reportUnknownMemberType=false
# pyright: reportUnknownArgumentType=false


from typing import Annotated, Any

import pydantic
from authlib.integrations.starlette_client import OAuth
from fastapi import Depends

from cbslib.auth import AuthError
from cbslib.config import get_config
from cbslib.config.server import GoogleOAuthSecrets

_SCOPES = [
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
]

_GOOGLE_CONF_URL = "https://accounts.google.com/.well-known/openid-configuration"


_oauth_config: GoogleOAuthSecrets | None = None
_oauth_client: OAuth | None = None


class GoogleOAuthUserInfo(pydantic.BaseModel):
    # skipped fields ...
    email: str
    name: str
    picture: str


class _GoogleOAuthToken(pydantic.BaseModel):
    # skipped fields ...
    userinfo: GoogleOAuthUserInfo


class AuthNoOAuthConfigError(AuthError):
    """OAuth config is missing."""

    def __init__(self) -> None:
        super().__init__("missing oauth config!")


class AuthNoOAuthClientError(AuthError):
    """OAuth client is missing."""

    def __init__(self) -> None:
        super().__init__("missing oauth client!")


class _GoogleInvalidTokenResponseError(AuthError):
    """Invalid Google Authentication Token response."""

    def __init__(self):
        super().__init__("invalid google token response")


def oauth_init_config() -> None:
    global _oauth_config
    config = get_config()

    assert config.server, "unexpected missing server config"
    _oauth_config = config.server.get_oauth_config()


def get_oauth_config() -> GoogleOAuthSecrets:
    if not _oauth_config:
        raise AuthNoOAuthConfigError()
    return _oauth_config


def oauth_init() -> None:
    global _oauth_client

    if _oauth_client:
        return

    oauth_init_config()
    #    config = get_config()
    oauth_config = get_oauth_config()

    oauth = OAuth()
    _ = oauth.register(
        name="google",
        client_id=oauth_config.client_id,
        client_secret=oauth_config.client_secret,
        server_metadata_url=_GOOGLE_CONF_URL,
        client_kwargs={
            # "scope": _SCOPES,
            "scope": "openid profile email",
        },
        prompt="select_account",
    )
    _oauth_client = oauth


def cbs_oauth() -> OAuth:
    if not _oauth_client:
        raise AuthNoOAuthClientError()
    return _oauth_client


def oauth_google_user_info(
    data: dict[str, Any],  # pyright: ignore[reportExplicitAny]
) -> GoogleOAuthUserInfo:
    try:
        token_res = _GoogleOAuthToken.model_validate(data)
    except pydantic.ValidationError:
        raise _GoogleInvalidTokenResponseError() from None

    return token_res.userinfo


CBSOAuth = Annotated[OAuth, Depends(cbs_oauth)]
