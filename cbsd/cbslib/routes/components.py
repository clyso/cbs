# CBS service library - routes - components
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


from fastapi import APIRouter

from cbsdcore.api.responses import BaseErrorModel
from cbslib.auth.users import CBSAuthUser
from cbslib.routes import logger as parent_logger
from cbslib.worker import tasks

_responses = {
    401: {
        "model": BaseErrorModel,
        "description": "Not authorized to perform request",
    },
    403: {
        "model": BaseErrorModel,
        "description": "User not authenticated",
    },
    500: {
        "model": BaseErrorModel,
        "description": "An internal error occurred, please check CBS logs",
    },
}

logger = parent_logger.getChild("components")

router = APIRouter(prefix="/components")


@router.get("/", responses={**_responses})
async def components_list(
    user: CBSAuthUser,
) -> list[str]:
    logger.debug(f"obtain components list, user: {user}")

    res = tasks.list_components.apply_async()
    components_lst = res.get()
    logger.debug(f"obtained components: {components_lst}")
    return components_lst
