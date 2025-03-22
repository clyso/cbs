# CBS - routes - authentication
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

# pyright: reportUnknownMemberType=false
# pyright: reportUnknownVariableType=false

# router
#
from typing import cast

from cbslib.auth import AuthError
from cbslib.auth.oauth import CBSOAuth, oauth_google_user_info
from cbslib.auth.users import CBSAuthUser, CBSAuthUsersDB
from cbslib.config.user import CBSUserConfig
from cbslib.routes import log as parent_logger
from fastapi import APIRouter, HTTPException, Request, Response, status
from fastapi.responses import RedirectResponse

log = parent_logger.getChild("auth")

router = APIRouter(prefix="/auth")


@router.get("/login")
async def auth_login(oauth: CBSOAuth, req: Request) -> RedirectResponse:
    log.debug(f"req base url: {req.base_url}")
    redirect_uri = req.url_for("auth_callback")  # takes function name
    log.debug(f"redirect uri: {redirect_uri}")
    redirect_uri_str = str(redirect_uri)

    google = cast(oauth.oauth2_client_cls, oauth.google)
    auth_uri = await google.create_authorization_url(
        redirect_uri_str,
        access_type="offline",
    )
    log.debug(f"auth uri: {auth_uri}")
    await google.save_authorize_data(
        req,
        redirect_uri=redirect_uri_str,
        **auth_uri,  # pyright: ignore[reportUnknownArgumentType]
    )

    log.debug(f"session: {req.session}")

    return RedirectResponse(
        auth_uri["url"],  # pyright: ignore[reportUnknownArgumentType]
        302,
    )


@router.get("/callback")
async def auth_callback(
    oauth: CBSOAuth, users: CBSAuthUsersDB, req: Request
) -> Response:
    google = cast(oauth.oauth2_client_cls, oauth.google)
    try:
        token = await google.authorize_access_token(req)
    except Exception as e:
        log.error(f"error authorizing access token: {e}")
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "unauthorized token")

    log.debug(f"token: {token}")

    try:
        user_info = oauth_google_user_info(
            token,  # pyright: ignore[reportUnknownArgumentType]
        )
    except AuthError as e:
        log.error(f"error obtaining google token: {e}")
        raise HTTPException(
            status.HTTP_401_UNAUTHORIZED, "error obtaining user information"
        )

    user = await users.create(user_info.email, user_info.name)
    user_config = CBSUserConfig(host=str(req.base_url), login_info=user)

    return Response(
        user_config.model_dump_json(indent=2),
        headers={
            "Content-Disposition": "attachment; filename=cbc-config.json",
        },
        media_type="application/json",
    )


@router.get("/whoami")
async def auth_whoami(user: CBSAuthUser) -> CBSAuthUser:
    log.debug(f"auth token info: {user}")
    return user


@router.get("/ping")
async def auth_ping() -> bool:
    return True
