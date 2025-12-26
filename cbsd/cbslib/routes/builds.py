# CBS server library - routes - builds
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


from typing import Any

from cbsdcore.api.responses import BaseErrorModel, NewBuildResponse
from cbsdcore.builds.types import BuildEntry, BuildID
from cbsdcore.versions import BuildDescriptor
from celery.result import AsyncResult
from fastapi import APIRouter, Depends, HTTPException, status
from fastapi.responses import JSONResponse

from cbslib.auth.caps import RequiredRouteCaps
from cbslib.auth.users import CBSAuthUser
from cbslib.builds.mgr import NotAvailableError
from cbslib.builds.tracker import (
    BuildExistsError,
    UnauthorizedTrackerError,
)
from cbslib.core.permissions import NotAuthorizedError, RoutesCaps
from cbslib.routes import logger as parent_logger
from cbslib.routes._utils import CBSBuildsMgr
from cbslib.worker.celery import celery_app

logger = parent_logger.getChild("builds")

router = APIRouter(prefix="/builds")


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


@router.post(
    "/new",
    responses={
        **_responses,
        200: {
            "model": BaseErrorModel,
            "description": "A build already exists for the same version",
        },
    },
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_BUILDS_NEW))],
)
async def builds_new(
    user: CBSAuthUser,
    mgr: CBSBuildsMgr,
    descriptor: BuildDescriptor,
) -> NewBuildResponse:
    logger.info(f"build new version: {descriptor}, user: {user}")

    user_info = descriptor.signed_off_by
    if user_info.email != user.email or user_info.user != user.name:
        logger.warning(f"unexpected user/email combination: {user_info}")
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="issuing user does not match authenticated user",
        )

    try:
        build_id, task_state = await mgr.new(user.email, descriptor)
    except NotAvailableError:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail="try again later"
        ) from None
    except NotAuthorizedError:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="User not authorized to perform requested build",
        ) from None
    except BuildExistsError as e:
        logger.info(f"build '{descriptor.version}' already exists")
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT, detail="Build already exists"
        ) from e
    except Exception as e:
        logger.error(f"unexpected error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        ) from e

    return NewBuildResponse(build_id=build_id, state=task_state)


@router.get(
    "/status",
    responses={**_responses},
)
async def get_builds_status(
    user: CBSAuthUser,
    mgr: CBSBuildsMgr,
    all: bool,
) -> list[tuple[BuildID, BuildEntry]]:
    logger.debug("obtain builds status for " + (f"{user.email}" if not all else "all"))

    owner = user.email if not all else None
    try:
        return await mgr.status(owner=owner)
    except Exception as e:
        logger.error(f"unexpected error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        ) from e


@router.get("/status/{task_id}")
async def get_task_status(task_id: str) -> JSONResponse:
    task_result = AsyncResult(task_id)  # pyright: ignore[reportUnknownVariableType]
    result = {  # pyright: ignore[reportUnknownVariableType]
        "task_id": task_id,
        "task_status": task_result.status,
        "task_result": task_result.result,  # pyright: ignore[reportUnknownMemberType]
    }
    return JSONResponse(result)


@router.delete(
    "/revoke/{build_id}",
    responses={**_responses},
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_BUILDS_REVOKE))],
)
async def revoke_build_id(
    user: CBSAuthUser,
    mgr: CBSBuildsMgr,
    build_id: BuildID,
    force: bool = False,
) -> bool:
    logger.debug(f"revoke task '{build_id}'")

    try:
        await mgr.revoke(build_id, user.email, force)
    except NotAvailableError:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail="try again later"
        ) from None
    except UnauthorizedTrackerError as e:
        logger.error(f"unable to revoke build '{build_id}': {e}")
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED, detail=str(e)
        ) from e
    except Exception as e:
        logger.error(f"unexpected error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        ) from e
    return True


@router.get(
    "/inspect",
    dependencies=[Depends(RequiredRouteCaps(RoutesCaps.ROUTES_BUILDS_INSPECT))],
)
async def get_status() -> JSONResponse:
    inspct = celery_app.control.inspect()

    active = inspct.active()
    scheduled = inspct.scheduled()
    reserved = inspct.reserved()

    active_info: list[tuple[str, str, Any]] = []  # pyright: ignore[reportExplicitAny]
    scheduled_info: list[Any] = []  # pyright: ignore[reportExplicitAny]
    reserved_info: list[Any] = []  # pyright: ignore[reportExplicitAny]

    if active:
        for tasks in active.values():
            active_info.extend(
                [(task["name"], task["id"], task["args"]) for task in tasks]
            )

    if scheduled:
        for tasks in scheduled.values():
            scheduled_info.extend(list(tasks))

    if reserved:
        for tasks in reserved.values():
            reserved_info.extend(list(tasks))

    return JSONResponse(
        {
            "active": active_info,
            "scheduled": scheduled_info,
            "reserved": reserved_info,
        }
    )
