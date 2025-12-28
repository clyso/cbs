# CBS service library - routes - utilities
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
from cbsdcore.auth.user import User
from fastapi import Depends, HTTPException, status

from cbslib.auth import AuthNoSuchUserError
from cbslib.auth.users import CBSAuthUsersDB
from cbslib.builds.mgr import BuildsMgr
from cbslib.core.mgr import CBSMgr
from cbslib.core.permissions import RoutesCaps
from cbslib.routes import logger as parent_logger
from cbslib.routes._auth import AuthTokenInfo, responses_auth_token

logger = parent_logger.getChild("utils")


responses_auth = {
    **responses_auth_token,
    401: {
        "description": "User not authorized to perform request",
        "model": BaseErrorModel,
    },
    403: {"description": "User not authenticated", "model": BaseErrorModel},
}


async def get_user(token_info: AuthTokenInfo, users: CBSAuthUsersDB) -> User:
    try:
        return await users.get_user(token_info.user)
    except AuthNoSuchUserError:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "Unauthorized user") from None


CBSAuthUser = Annotated[User, Depends(get_user)]


responses_builds = {
    503: {
        "description": "Service hasn't started yet, try again later",
        "model": BaseErrorModel,
    }
}


def get_builds_mgr(mgr: CBSMgr) -> BuildsMgr:
    builds_mgr = mgr.builds_mgr
    if not builds_mgr.available:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="Service hasn't started yet, try again later.",
        ) from None
    return builds_mgr


CBSBuildsMgr = Annotated[BuildsMgr, Depends(get_builds_mgr)]


responses_caps = {
    **responses_auth,
    403: {
        "description": "User missing required capabilities",
        "model": BaseErrorModel,
    },
}


class RequiredRouteCaps:
    _required: RoutesCaps

    def __init__(self, required: RoutesCaps) -> None:
        self._required = required

    def __call__(self, user: CBSAuthUser, mgr: CBSMgr) -> None:
        logger.debug(f"checking user '{user.email}' for caps '{self._required}'")
        if not mgr.permissions.is_authorized_for_route(user.email, self._required):
            logger.warning(
                f"authorization failed for user '{user.email}' "
                + f"missing caps '{self._required}'"
            )
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="User missing required capabilities",
            )
