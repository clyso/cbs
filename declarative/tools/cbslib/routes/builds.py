# CBS - routes - builds
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

from cbslib.auth.users import CBSAuthUser
from cbslib.builds.tracker import (
    BuildExistsError,
    CBSBuildsTracker,
    UnauthorizedTrackerError,
)
from cbslib.builds.types import BuildEntry
from cbslib.routes import log as parent_logger
from cbslib.routes.models import BaseErrorModel, NewBuildResponse
from cbslib.worker.celery import celery_app
from celery.result import AsyncResult
from ceslib.versions.desc import VersionDescriptor
from fastapi import APIRouter, HTTPException, status
from fastapi.responses import JSONResponse

log = parent_logger.getChild("builds")

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
)
async def builds_new(
    user: CBSAuthUser,
    tracker: CBSBuildsTracker,
    descriptor: VersionDescriptor,
) -> NewBuildResponse:
    log.debug(f"build new version: {descriptor}, user: {user}")

    user_info = descriptor.signed_off_by
    if user_info.email != user.email or user_info.user != user.name:
        log.error(f"unexpected user/email combination: {user_info}")
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="issuing user does not match token's user",
        )

    try:
        task_id, task_state = await tracker.new(descriptor)
    except BuildExistsError as e:
        log.error(f"build exists: {e}")
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT, detail="Build already exists"
        )
    except Exception as e:
        log.error(f"unexpected error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        )

    return NewBuildResponse(task_id=task_id, state=task_state)


@router.get("/status", responses={**_responses})
async def get_builds_status(
    user: CBSAuthUser,
    tracker: CBSBuildsTracker,
    all: bool,
) -> list[BuildEntry]:
    log.debug("obtain builds status for " + (f"{user.email}" if not all else "all"))

    owner = user.email if not all else None
    try:
        return await tracker.builds(owner)
    except Exception as e:
        log.error(f"unexpected error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        )


@router.delete("/abort/{build_id}", responses={**_responses})
async def delete_build_id(
    user: CBSAuthUser,
    tracker: CBSBuildsTracker,
    build_id: str,
    force: bool = False,
) -> bool:
    log.debug(f"abort task '{build_id}'")

    try:
        await tracker.abort_build(build_id, user.email, force)
        return True
    except UnauthorizedTrackerError as e:
        log.error(f"unable to abort build '{build_id}': {e}")
        raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail=str(e))
    except Exception as e:
        log.error(f"unexpected error: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="check logs for failure",
        )


@router.get("/status/{task_id}")
async def get_task_status(task_id: str) -> JSONResponse:
    task_result = AsyncResult(task_id)  # pyright: ignore[reportUnknownVariableType]
    result = {  # pyright: ignore[reportUnknownVariableType]
        "task_id": task_id,
        "task_status": task_result.status,
        "task_result": task_result.result,  # pyright: ignore[reportUnknownMemberType]
    }
    return JSONResponse(result)


@router.get("/inspect")
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
            scheduled_info.extend([task for task in tasks])

    if reserved:
        for tasks in reserved.values():
            reserved_info.extend([task for task in tasks])

    return JSONResponse(
        {
            "active": active_info,
            "scheduled": scheduled_info,
            "reserved": reserved_info,
        }
    )
