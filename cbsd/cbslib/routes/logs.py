# CBS service library - routes - logs
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

import fastapi
from cbsdcore.api.responses import BaseErrorModel, BuildLogsFollowResponse
from cbsdcore.builds.types import BuildID
from fastapi import APIRouter, Depends, HTTPException, status

from cbslib.builds.logs import BuildLogsHandlerError
from cbslib.core import utils
from cbslib.core.permissions import RoutesCaps
from cbslib.routes import logger as parent_logger
from cbslib.routes._utils import (
    CBSBuildsMgr,
    RequiredRouteCaps,
    responses_auth,
    responses_caps,
)

logger = parent_logger.getChild("logs")

router = APIRouter(prefix="/logs")


@router.get(
    "/{build_id}/tail",
    summary="Obtains the tail of the log for a given build",
    responses={
        **responses_auth,
        **responses_caps,
        500: {
            "model": BaseErrorModel,
            "description": "An internal error occurred, please check CBS service logs",
        },
        404: {
            "model": BaseErrorModel,
            "description": "Build not found",
        },
        200: {
            "model": list[str],
            "description": "List of log messages for the given build",
        },
    },
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_BUILDS_STATUS))],
)
async def get_build_logs_tail(
    mgr: CBSBuildsMgr,
    build_id: Annotated[BuildID, fastapi.Path(description="Build's ID")],
    n: Annotated[int, fastapi.Query(description="Number of lines to return")] = 30,
) -> BuildLogsFollowResponse:
    logger.debug(f"tail build log '{build_id}' max '{n}'")
    try:
        end_of_stream, last_id, res = await mgr.logs.tail(build_id, n)
    except BuildLogsHandlerError as e:
        logger.error(f"error handling follow request: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check service logs for failure",
        ) from e
    except utils.FileNotFoundError:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="build not found",
        ) from None
    return BuildLogsFollowResponse(
        last_id=last_id, msgs=res, end_of_stream=end_of_stream
    )


@router.get(
    "/{build_id}/follow",
    summary="Follows the tail of a given build's log",
    responses={
        **responses_auth,
        **responses_caps,
        500: {
            "model": BaseErrorModel,
            "description": "An internal error occurred, please check CBS service logs",
        },
        404: {
            "model": BaseErrorModel,
            "description": "Build not found",
        },
        200: {
            "model": list[str],
            "description": "List of log messages for the given build",
        },
    },
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_BUILDS_STATUS))],
)
async def get_build_logs_follow(
    mgr: CBSBuildsMgr,
    build_id: Annotated[BuildID, fastapi.Path(description="Build's ID")],
    n: Annotated[int, fastapi.Query(description="Number of lines to return")] = 30,
    since: Annotated[
        str | None, fastapi.Query(description="Last message ID seen")
    ] = None,
) -> BuildLogsFollowResponse:
    logger.debug(f"follow build log '{build_id}, since '{since}', max '{n}'")
    try:
        end_of_stream, last_id, res = await mgr.logs.follow(build_id, since, max_msgs=n)
    except BuildLogsHandlerError as e:
        logger.error(f"error handling follow request: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check service logs for failure",
        ) from e
    except utils.FileNotFoundError:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="build not found",
        ) from None

    return BuildLogsFollowResponse(
        last_id=last_id, msgs=res, end_of_stream=end_of_stream
    )
