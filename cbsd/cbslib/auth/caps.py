# CBS service library - auth library - caps
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


from fastapi import HTTPException, status

from cbslib.auth import logger as parent_logger
from cbslib.auth.users import CBSAuthUser
from cbslib.core.mgr import CBSMgr
from cbslib.core.permissions import RoutesCaps

logger = parent_logger.getChild("caps")


class RequiredRouteCaps:
    _required: RoutesCaps

    def __init__(self, required: RoutesCaps) -> None:
        self._required = required

    def __call__(self, user: CBSAuthUser, mgr: CBSMgr) -> None:
        logger.debug(f"checking user '{user.email}' for caps '{self._required}'")
        if not mgr.permissions.is_authorized_for_route(user.email, self._required):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="User missing required capabilities",
            )
