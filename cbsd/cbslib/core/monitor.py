# CBS server library - workqueue's worker - event monitor
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
import datetime
import threading
from collections.abc import Callable, Coroutine
from datetime import datetime as dt
from typing import Any, Concatenate, ParamSpec, TypeVar

import celery
import celery.events  # pyright: ignore[reportMissingImports]
import kombu
import pydantic

from cbslib.builds.tracker import BuildsTracker
from cbslib.logger import logger as parent_logger
from cbslib.worker.celery import celery_app

logger = parent_logger.getChild("monitor")

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


_ModelFn = Callable[Concatenate[BuildsTracker, _BM, _P], Coroutine[None, None, _R]]
_DictFn = Callable[Concatenate[_EventDict, _P], _R]


def _with_tracker(
    tracker: BuildsTracker,
    bm: type[_BM],
    fn: _ModelFn[_BM, _P, _R],
    event_loop: asyncio.AbstractEventLoop,
) -> _DictFn[_P, _R]:
    def wrapper(e: _EventDict, *args: _P.args, **kwargs: _P.kwargs) -> _R:
        m = bm(**e)
        loop = event_loop
        logger.info(f"running '{fn}' in loop '{loop}'")
        try:
            ftr = asyncio.run_coroutine_threadsafe(
                fn(tracker, m, *args, **kwargs), loop
            )
        except Exception as exc:
            logger.error(f"unable to run '{fn}' in loop '{loop}': {exc}")
            raise exc from None
        return ftr.result()

    return wrapper


async def _event_task_started(tracker: BuildsTracker, event: _EventTaskStarted) -> None:
    logger.info(f"task started: uuid = {event.uuid}, ts = {event.timestamp}")
    await tracker.mark_started(
        event.uuid, dt.fromtimestamp(event.timestamp, datetime.UTC)
    )


async def _event_task_succeeded(
    tracker: BuildsTracker, event: _EventTaskSucceeded
) -> None:
    logger.info(f"task succeeded, uuid: {event.uuid}, runtime: {event.runtime}")
    await tracker.mark_succeeded(
        event.uuid, dt.fromtimestamp(event.timestamp, datetime.UTC)
    )


async def _event_task_failed(tracker: BuildsTracker, event: _EventTaskFailed) -> None:
    logger.info(
        f"task failed, uuid: {event.uuid}, exception: {event.exception}, "
        + f"ts: {event.timestamp}"
    )
    await tracker.mark_failed(
        event.uuid, dt.fromtimestamp(event.timestamp, datetime.UTC)
    )


async def _event_task_rejected(
    tracker: BuildsTracker, event: _EventTaskRejected
) -> None:
    logger.info(f"task rejected, uuid: {event.uuid}")
    await tracker.mark_rejected(event.uuid)


async def _event_task_revoked(tracker: BuildsTracker, event: _EventTaskRevoked) -> None:
    logger.info(
        f"task revoked, uuid: {event.uuid}, terminated: {event.terminated}, "
        + f"signum: {event.signum}, expired: {event.expired}"
    )
    await tracker.mark_revoked(event.uuid)


class Monitor:
    """Monitors task events from the celery workqueue."""

    _builds_tracker: BuildsTracker
    _thread: threading.Thread | None
    _connection: kombu.Connection | None
    _receiver: celery.events.EventReceiver | None
    _event_loop: asyncio.AbstractEventLoop

    def __init__(
        self, builds_tracker: BuildsTracker, event_loop: asyncio.AbstractEventLoop
    ) -> None:
        self._builds_tracker = builds_tracker
        self._thread = None
        self._connection = None
        self._receiver = None
        self._event_loop = event_loop

    def start(self) -> None:
        if self._thread:
            logger.warning("monitoring already started")
            return

        self._thread = threading.Thread(target=self._do_monitoring)
        self._thread.start()

    def _do_monitoring(self) -> None:
        logger.info("starting task monitoring")
        if self._connection:
            logger.warning("monitoring already started")
            return

        asyncio.set_event_loop(self._event_loop)

        try:
            self._connection = celery_app.connection()
        except Exception as e:
            logger.error(f"error creating connection for monitoring: {e}")
            return

        self._receiver = celery_app.events.Receiver(
            self._connection,
            handlers={
                "task-started": _with_tracker(
                    self._builds_tracker,
                    _EventTaskStarted,
                    _event_task_started,
                    self._event_loop,
                ),
                "task-succeeded": _with_tracker(
                    self._builds_tracker,
                    _EventTaskSucceeded,
                    _event_task_succeeded,
                    self._event_loop,
                ),
                "task-failed": _with_tracker(
                    self._builds_tracker,
                    _EventTaskFailed,
                    _event_task_failed,
                    self._event_loop,
                ),
                "task-rejected": _with_tracker(
                    self._builds_tracker,
                    _EventTaskRejected,
                    _event_task_rejected,
                    self._event_loop,
                ),
                "task-revoked": _with_tracker(
                    self._builds_tracker,
                    _EventTaskRevoked,
                    _event_task_revoked,
                    self._event_loop,
                ),
            },
        )

        assert self._receiver is not None
        try:
            self._receiver.capture(limit=None, timeout=None, wakeup=None)
        except Exception as e:
            logger.error(f"error capturing events: {e}")
            return
        finally:
            self._connection.release()
            self._connection = None

    def stop(self) -> None:
        logger.info("stopping task monitoring")
        if not self._thread:
            return
        if not self._receiver:
            logger.warning("monitoring thread exists, receiver missing")
            return
        self._receiver.should_stop = True
        self._thread.join()
        self._thread = None
