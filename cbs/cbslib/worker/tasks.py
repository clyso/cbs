# CBS - workqueue's worker - tasks
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

import asyncio
from typing import Any, ParamSpec, cast, override

from cbslib.worker.builder import WorkerBuilder, WorkerBuilderError
from cbslib.worker.celery import celery_app, log
from celery import Task
from celery.worker.request import Request
from ceslib.versions.desc import VersionDescriptor

Task.__class_getitem__ = classmethod(  # pyright: ignore[reportAttributeAccessIssue]
    lambda cls, *args, **kwargs: cls,
)

_P = ParamSpec("_P")


class BuilderRequest(Request):
    @override
    def terminate(
        self,
        pool: Any,  # pyright: ignore[reportExplicitAny,reportAny]
        signal: int | None = None,
    ) -> None:
        log.info(f"request terminated: {self.task_id}, signal: {signal}")
        super().terminate(pool, signal)  # pyright: ignore[reportAny]
        task = cast("BuilderTask[None]", self.task)
        task.on_termination(self.task_id)


class BuilderTask(Task[_P, None]):
    Request = BuilderRequest  # pyright: ignore[reportUnannotatedClassAttribute]

    builder: WorkerBuilder

    def __init__(self) -> None:
        super().__init__()
        self.builder = WorkerBuilder()

    def on_termination(self, task_id: str) -> None:
        log.info(f"revoked {task_id}")
        loop = asyncio.new_event_loop()
        loop.run_until_complete(self.builder.kill())


@celery_app.task(pydantic=True, base=BuilderTask, bind=True, track_started=True)
def build(self: BuilderTask[None], version_desc: VersionDescriptor) -> None:
    log.info(f"build version: {version_desc}")

    loop = asyncio.new_event_loop()
    try:
        # loop.run_until_complete(asyncio.sleep(120))
        loop.run_until_complete(self.builder.build(version_desc))
    except (WorkerBuilderError, Exception) as e:
        log.exception("error running build")
        raise e  # noqa: TRY201
