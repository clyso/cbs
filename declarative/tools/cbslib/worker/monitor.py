# CBS - workqueue's worker - event monitor
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

from functools import wraps
from typing import Any, Callable, Concatenate, ParamSpec, TypeVar

import pydantic
from cbslib.logger import log as parent_logger
from cbslib.worker.celery import celery_app

log = parent_logger.getChild("monitor")

# monitoring as seen in
#   https://docs.celeryq.dev/en/stable/userguide/monitoring.html
#

# pyright: reportAttributeAccessIssue=false
# pyright: reportUnknownMemberType=false
# pyright: reportUnknownVariableType=false

_EventDict = dict[str, Any]  # pyright: ignore[reportExplicitAny]
_P = ParamSpec("_P")
_R = TypeVar("_R")
_BM = TypeVar("_BM", bound=pydantic.BaseModel)

_ModelFn = Callable[Concatenate[_BM, _P], _R]
_DictFn = Callable[Concatenate[_EventDict, _P], _R]
_ToModelFnWrapper = Callable[[_ModelFn[_BM, _P, _R]], _DictFn[_P, _R]]


def _as_model(bm: type[_BM]) -> _ToModelFnWrapper[_BM, _P, _R]:
    def inner(fn: _ModelFn[_BM, _P, _R]) -> _DictFn[_P, _R]:
        @wraps(fn)
        def wrapper(e: _EventDict, *args: _P.args, **kwargs: _P.kwargs) -> _R:
            m = bm(**e)
            return fn(m, *args, **kwargs)

        return wrapper

    return inner


class _EventTaskStarted(pydantic.BaseModel):
    uuid: str
    timestamp: float


class _EventTaskSucceeded(pydantic.BaseModel):
    uuid: str
    result: Any  # pyright: ignore[reportExplicitAny]
    runtime: float
    timestamp: float


class _EventTaskFailed(pydantic.BaseModel):
    uuid: str
    exception: str
    timestamp: float


class _EventTaskRejected(pydantic.BaseModel):
    uuid: str


class _EventTaskRevoked(pydantic.BaseModel):
    uuid: str
    terminated: bool
    signum: int
    expired: bool


@_as_model(_EventTaskStarted)
def _event_task_started(event: _EventTaskStarted) -> None:
    log.info(f"task started: uuid = {event.uuid}, ts = {event.timestamp}")


@_as_model(_EventTaskSucceeded)
def _event_task_succeeded(event: _EventTaskSucceeded) -> None:
    log.info(f"task succeeded, uuid: {event.uuid}, runtime: {event.runtime}")


@_as_model(_EventTaskFailed)
def _event_task_failed(event: _EventTaskFailed) -> None:
    log.info(
        f"task failed, uuid: {event.uuid}, exception: {event.exception}, "
        + f"ts: {event.timestamp}"
    )


@_as_model(_EventTaskRejected)
def _event_task_rejected(event: _EventTaskRejected) -> None:
    log.info(f"task rejected, uuid: {event.uuid}")


@_as_model(_EventTaskRevoked)
def _event_task_revoked(event: _EventTaskRevoked) -> None:
    log.info(
        f"task revoked, uuid: {event.uuid}, terminated: {event.terminated}, "
        + f"signum: {event.signum}, expired: {event.expired}"
    )


def monitor() -> None:
    try:
        with celery_app.connection() as conn:
            recv = celery_app.events.Receiver(
                conn,
                handlers={
                    "task-started": _event_task_started,
                    "task-succeeded": _event_task_succeeded,
                    "task-failed": _event_task_failed,
                    "task-rejected": _event_task_rejected,
                    "task-revoked": _event_task_revoked,
                },
            )
            recv.capture(limit=None, timeout=None, wakeup=None)
    except Exception as e:
        log.error(f"error capturing events: {e}")
        pass
    pass
