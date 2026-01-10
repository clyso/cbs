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

from __future__ import annotations

from typing import Any, ParamSpec, override

import pydantic
from cbscore.core.component import load_components
from cbsdcore.api.responses import AvailableComponent
from cbsdcore.builds.types import BuildID
from cbsdcore.versions import BuildDescriptor
from celery import Task
from celery.worker.request import Request

from cbslib.config.config import get_config
from cbslib.worker import WorkerError
from cbslib.worker.builder import WorkerBuilderError, get_builder
from cbslib.worker.celery import celery_app, logger
from cbslib.worker.worker import get_worker

Task.__class_getitem__ = classmethod(  # pyright: ignore[reportAttributeAccessIssue]
    lambda cls, *args, **kwargs: cls,
)

_P = ParamSpec("_P")


class ListComponentsTaskResponse(pydantic.BaseModel):
    components: dict[str, AvailableComponent]


class BuilderRequest(Request):
    """Defines a request for a given worker's builder task."""

    @override
    def on_accepted(self, pid: str, time_accepted: float) -> None:
        logger.info(f"request accepted: {self.task_id}, pid: {pid}")
        return super().on_accepted(pid, time_accepted)

    @override
    def terminate(
        self,
        pool: Any,  # pyright: ignore[reportExplicitAny,reportAny]
        signal: int | None = None,
    ) -> None:
        logger.info(f"request terminated: {self.task_id}, signal: {signal}")
        super().terminate(pool, signal)  # pyright: ignore[reportAny]
        worker = get_worker()
        worker.terminate_build(self.task_id)


class BuilderTask(Task[_P, None]):
    """Task for a given worker's builds."""

    Request = BuilderRequest  # pyright: ignore[reportUnannotatedClassAttribute]


@celery_app.task(pydantic=True, base=BuilderTask, bind=True, track_started=True)
def build(
    self: BuilderTask[None], build_id: BuildID, build_desc: BuildDescriptor
) -> None:
    task_id = self.request.id
    logger.info(
        f"build id '{build_id}', task id: {task_id}:\n"
        + f"{build_desc.model_dump_json(indent=2)}"
    )

    assert task_id, "unexpected missing request id for task"

    builder = get_builder()
    try:
        builder.build(task_id, build_id, build_desc)
    except (WorkerBuilderError, Exception) as e:
        logger.error(f"error running build: {e}")
        raise e from None


@celery_app.task(pydantic=True)
def list_components() -> ListComponentsTaskResponse:
    logger.debug("list components")

    config = get_config()
    if not config.worker:
        msg = "unexpected missing worker config"
        logger.error(msg)
        raise WorkerError(msg)

    cbscore_config = config.worker.get_cbscore_config()

    avail_components: dict[str, AvailableComponent] = {}
    avail_components_map = load_components(
        cbscore_config.paths.components,
    )
    for comp_name, comp_loc in avail_components_map.items():
        ctr_path = comp_loc.path / comp_loc.comp.containers.path
        if not ctr_path.exists() or not ctr_path.is_dir():
            logger.warning(
                f"missing containers path '{ctr_path}' for component '{comp_name}'"
            )
            continue

        avail_versions: list[str] = []
        for p in list(ctr_path.rglob("container.yaml")):
            avail_versions.append("*" if p.parent == ctr_path else p.parent.name)

        if not avail_versions:
            logger.warning(
                f"no container versions found for component '{comp_name}' "
                + f"in '{ctr_path}'"
            )
            continue

        avail_components[comp_name] = AvailableComponent(
            name=comp_name,
            default_repo=comp_loc.comp.repo,
            versions=avail_versions,
        )

    logger.debug(f"obtain available components: {avail_components}")
    return ListComponentsTaskResponse(components=avail_components)
