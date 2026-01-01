# CBS server library - routes - authentication
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

# pyright: reportUnknownMemberType=false
# pyright: reportUnknownVariableType=false

# router
#
from typing import cast

import pydantic
from cbsdcore.api.responses import BaseErrorModel
from cbsdcore.auth.user import User, UserConfig
from fastapi import APIRouter, Depends, HTTPException, Request, Response, status
from fastapi.responses import RedirectResponse

from cbslib.auth import AuthError
from cbslib.auth.oauth import CBSOAuth, oauth_google_user_info
from cbslib.auth.users import CBSAuthUsersDB
from cbslib.core.mgr import CBSMgr
from cbslib.core.permissions import AuthorizationEntry, RoutesCaps
from cbslib.routes import logger as parent_logger
from cbslib.routes._utils import (
    CBSAuthUser,
    RequiredRouteCaps,
    responses_auth,
    responses_caps,
)

logger = parent_logger.getChild("auth")

router = APIRouter(prefix="/auth")
permissions_router = APIRouter(prefix="/auth/permissions")


@router.get(
    "/login",
    summary="Start login process",
)
async def auth_login(oauth: CBSOAuth, req: Request) -> RedirectResponse:
    """Start the login process, redirecting the user to google's SSO."""
    logger.debug(f"req base url: {req.base_url}")
    redirect_uri = req.url_for("auth_callback")  # takes function name
    logger.debug(f"redirect uri: {redirect_uri}")
    redirect_uri_str = str(redirect_uri)

    google = cast(oauth.oauth2_client_cls, oauth.google)
    auth_uri = await google.create_authorization_url(
        redirect_uri_str,
        access_type="offline",
    )
    logger.debug(f"auth uri: {auth_uri}")
    await google.save_authorize_data(
        req,
        redirect_uri=redirect_uri_str,
        **auth_uri,  # pyright: ignore[reportUnknownArgumentType]
    )

    logger.debug(f"session: {req.session}")

    return RedirectResponse(
        auth_uri["url"],  # pyright: ignore[reportUnknownArgumentType]
        302,
    )


@router.get(
    "/callback",
    summary="Authentication callback from SSO",
    responses={
        401: {"description": "User not authorized", "model": BaseErrorModel},
        200: {"description": "Authorized user", "model": UserConfig},
    },
)
async def auth_callback(
    oauth: CBSOAuth, users: CBSAuthUsersDB, req: Request
) -> Response:
    """
    Handle the redirect from the SSO service.

    Will handle the callback token issued by the Google SSO,
    thus authorizing the user.

    In case of success, will redirect the user to download a configuration
    file to interact with the build service.
    """
    google = cast(oauth.oauth2_client_cls, oauth.google)
    try:
        token = await google.authorize_access_token(req)
    except Exception as e:
        logger.exception("error authorizing access token")
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "unauthorized token") from e

    logger.debug(f"token: {token}")

    try:
        user_info = oauth_google_user_info(
            token,  # pyright: ignore[reportUnknownArgumentType]
        )
    except AuthError as e:
        logger.exception("error obtaining google token")
        raise HTTPException(
            status.HTTP_401_UNAUTHORIZED, "error obtaining user information"
        ) from e

    user = await users.create(user_info.email, user_info.name)
    user_config = UserConfig(host=str(req.base_url), login_info=user)

    return Response(
        user_config.model_dump_json(indent=2),
        headers={
            "Content-Disposition": "attachment; filename=cbc-config.json",
        },
        media_type="application/json",
    )


@router.get(
    "/whoami",
    summary="Returns information about the authenticated user",
    responses={
        **responses_auth,
        200: {
            "description": "User information",
            "model": User,
        },
    },
)
async def auth_whoami(user: CBSAuthUser) -> User:
    """Return information about the authenticated user, if any."""
    logger.debug(f"auth token info: {user}")
    return user


@router.get("/ping")
async def auth_ping() -> bool:
    return True


# permissions
#


class _UserPermissionsListResponse(pydantic.BaseModel):
    authorizations: list[AuthorizationEntry]
    from_groups: dict[str, list[AuthorizationEntry]]


@permissions_router.get(
    "/",
    summary="List the user's known capabilities",
    responses={
        **responses_caps,
        200: {
            "description": "User's capabilities",
            "model": _UserPermissionsListResponse,
        },
    },
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_AUTH_PERMISSIONS))],
)
def auth_permissions_list(
    user: CBSAuthUser, mgr: CBSMgr
) -> _UserPermissionsListResponse:
    """Return the user's known capabilities."""
    authorizations, from_groups = mgr.permissions.list_caps_for(user.email)

    return _UserPermissionsListResponse(
        authorizations=authorizations,
        from_groups=from_groups,
    )
