# CBS server library - workqueue's worker - tasks
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

from celery import Task
from celery.worker.request import Request

from cbsdcore.versions import BuildDescriptor
from cbslib.config.config import get_config
from cbslib.worker import WorkerError
from cbslib.worker.builder import WorkerBuilderError, get_builder
from cbslib.worker.celery import celery_app, logger

Task.__class_getitem__ = classmethod(  # pyright: ignore[reportAttributeAccessIssue]
    lambda cls, *args, **kwargs: cls,
)

_P = ParamSpec("_P")


class BuilderRequest(Request):
    """Defines a request for a given worker's builder task."""

    @override
    def terminate(
        self,
        pool: Any,  # pyright: ignore[reportExplicitAny,reportAny]
        signal: int | None = None,
    ) -> None:
        logger.info(f"request terminated: {self.task_id}, signal: {signal}")
        super().terminate(pool, signal)  # pyright: ignore[reportAny]
        task = cast("BuilderTask[None]", self.task)
        task.on_termination(self.task_id)


class BuilderTask(Task[_P, None]):
    """Task for a given worker's builds."""

    Request = BuilderRequest  # pyright: ignore[reportUnannotatedClassAttribute]

    def __init__(self) -> None:
        super().__init__()
        logger.info(f"initted task {self.name}")

    def on_termination(self, task_id: str) -> None:
        logger.info(f"revoked {task_id}")
        builder = get_builder()
        loop = asyncio.new_event_loop()
        loop.run_until_complete(builder.kill())


@celery_app.task(pydantic=True, base=BuilderTask, bind=True, track_started=True)
def build(_self: BuilderTask[None], build_desc: BuildDescriptor) -> None:
    logger.info(f"build descriptor: {build_desc}")

    builder = get_builder()
    loop = asyncio.new_event_loop()
    try:
        loop.run_until_complete(builder.build(build_desc))
    except (WorkerBuilderError, Exception) as e:
        logger.exception("error running build")
        raise e  # noqa: TRY201


@celery_app.task(pydantic=True)
def list_components() -> list[str]:
    logger.debug("list components")

    config = get_config()
    if not config.worker:
        msg = "unexpected missing worker config"
        logger.error(msg)
        raise WorkerError(msg)

    cbscore_config = config.worker.get_cbscore_config()

    avail_components: list[str] = []
    for components_path in cbscore_config.paths.components:
        for candidate in components_path.iterdir():
            if not candidate.is_dir():
                continue
            avail_components.append(candidate.name)

    logger.debug(f"obtain available components: {avail_components}")
    return avail_components
