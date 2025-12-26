# CBS service library - routes - periodic tasks
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


import uuid

from cbsdcore.api.requests import NewPeriodicBuildTaskRequest
from cbsdcore.api.responses import BaseErrorModel
from fastapi import APIRouter, Depends, HTTPException, status

from cbslib.core.periodic import BadCronFormatError, PeriodicTrackerError
from cbslib.core.permissions import RoutesCaps
from cbslib.routes import logger as parent_logger
from cbslib.routes._utils import CBSAuthUser, CBSPeriodicTracker, RequiredRouteCaps

logger = parent_logger.getChild("periodic")

router = APIRouter(prefix="/periodic")


_responses = {
    400: {"description": "Invalid request data", "model": BaseErrorModel},
    401: {"description": "Not authorized to perform request"},
    403: {"description": "User not authenticated"},
}


@router.post(
    "/build",
    summary="Create new periodic build task",
    responses={
        **_responses,
        200: {"description": "Periodic build task created"},
    },
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_PERIODIC_BUILDS_NEW))],
)
async def periodic_build_task(
    user: CBSAuthUser,
    tracker: CBSPeriodicTracker,
    req: NewPeriodicBuildTaskRequest,
) -> uuid.UUID:
    """Create a new periodic build task."""
    logger.info(f"creating new periodic build task, user: {user.email}")

    if not req.cron_format:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="missing 'cron format' information",
        )
    elif not req.tag_format:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="missing 'tag format' information",
        )

    try:
        cron_uuid = await tracker.add_build_task(
            cron_format=req.cron_format,
            tag_format=req.tag_format,
            created_by=user.email,
            descriptor=req.descriptor,
            summary=req.summary,
        )
    except BadCronFormatError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=str(e),
        ) from e
    except PeriodicTrackerError as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR, detail=str(e)
        ) from e
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"unexpected error: {e}",
        ) from e

    return cron_uuid
