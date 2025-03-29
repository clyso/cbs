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
from cbslib.routes import log as parent_logger
from cbslib.worker.celery import celery_app
from cbslib.worker.tasks import build
from celery.result import AsyncResult
from ceslib.versions.desc import VersionDescriptor
from fastapi import APIRouter
from fastapi.responses import JSONResponse

log = parent_logger.getChild("builds")

router = APIRouter(prefix="/builds")


@router.post("/new")
async def builds_new(
    user: CBSAuthUser,
    descriptor: VersionDescriptor,
) -> JSONResponse:
    log.debug(f"build new version: {descriptor}, user: {user}")
    task = build.apply_async(
        (descriptor,), serializer="pydantic"
    )  #    task = build.delay(descriptor.model_dump())
    log.info(f"building, task id: {task.id}")
    return JSONResponse({"task_id": task.id})


@router.get("/status/{task_id}")
async def get_task_status(task_id: str) -> JSONResponse:
    task_result = AsyncResult(task_id)  # pyright: ignore[reportUnknownVariableType]
    result = {  # pyright: ignore[reportUnknownVariableType]
        "task_id": task_id,
        "task_status": task_result.status,
        "task_result": task_result.result,  # pyright: ignore[reportUnknownMemberType]
    }
    return JSONResponse(result)


@router.get("/status")
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
