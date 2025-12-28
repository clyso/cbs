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


from cbsdcore.api.responses import AvailableComponentsResponse, BaseErrorModel
from fastapi import APIRouter, HTTPException, status

from cbslib.builds.mgr import NotAvailableError
from cbslib.routes import logger as parent_logger
from cbslib.routes._utils import CBSAuthUser, CBSBuildsMgr, responses_auth

logger = parent_logger.getChild("components")

router = APIRouter(prefix="/components")


@router.get(
    "/",
    summary="Obtain known build components",
    responses={
        **responses_auth,
        500: {
            "description": "Unknown error obtaining components",
            "model": BaseErrorModel,
        },
        200: {
            "description": "Known components",
            "model": AvailableComponentsResponse,
        },
    },
)
async def components_list(
    user: CBSAuthUser,
    mgr: CBSBuildsMgr,
) -> AvailableComponentsResponse:
    """Obtain known components that are able to be built by the service."""
    logger.debug(f"obtain components list, user: {user}")

    try:
        return mgr.components
    except NotAvailableError:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail="try again later"
        ) from None
    except Exception as e:
        logger.error(f"unknown error obtaining components: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        ) from e
