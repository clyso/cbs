# CBS server library - builds - tracker
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
from datetime import datetime as dt
from typing import Annotated, override

from celery.result import AsyncResult as CeleryTaskResult
from fastapi import Depends

from cbscore.errors import CESError
from cbscore.versions.desc import VersionDescriptor
from cbslib.builds import logger as parent_logger
from cbslib.builds.types import BuildEntry, EntryState
from cbslib.worker import tasks

logger = parent_logger.getChild("tracker")


class TrackerError(CESError):
    @override
    def __str__(self) -> str:
        return "Tracker Error" + (f": {self.msg}" if self.msg else "")


class BuildExistsError(TrackerError):
    @override
    def __str__(self) -> str:
        return "Build Exists Error" + (f": {self.msg}" if self.msg else "")


class NoSuchBuildError(TrackerError):
    @override
    def __str__(self) -> str:
        return "No Such Build Error" + (f": {self.msg}" if self.msg else "")


class UnauthorizedTrackerError(TrackerError):
    _user: str
    _op: str

    def __init__(self, user: str, op: str) -> None:
        super().__init__()
        self._user = user
        self._op = op

    @override
    def __str__(self) -> str:
        return (
            f"Unauthorized Tracker Error: user '{self._user}' "
            + f"not authorized to perform '{self._op}'"
        )


class BuildsTracker:
    _builds: dict[str, BuildEntry]
    _builds_by_version: dict[str, list[BuildEntry]]
    _lock: asyncio.Lock

    def __init__(self) -> None:
        self._builds = {}
        self._builds_by_version = {}
        self._lock = asyncio.Lock()

    async def new(self, desc: VersionDescriptor) -> tuple[str, str]:
        _ = await self._lock.acquire()
        try:
            if desc.version in self._builds_by_version and any(
                map(  # noqa: C417
                    lambda x: not x.finished, self._builds_by_version[desc.version]
                )
            ):
                raise BuildExistsError(desc.version)

            task = tasks.build.apply_async((desc,), serializer="pydantic")

            build_entry = BuildEntry(
                task_id=task.task_id,
                desc=desc,
                user=desc.signed_off_by.email,
                submitted=dt.now(tz=datetime.UTC),
                state=EntryState(task.state.upper()),
                started=None,
                finished=None,
            )
            self._builds[build_entry.task_id] = build_entry
            if desc.version not in self._builds_by_version:
                self._builds_by_version[desc.version] = []

            self._builds_by_version[desc.version].append(build_entry)

            return (build_entry.task_id, build_entry.state)
        finally:
            self._lock.release()

    async def list(
        self, *, owner: str | None = None, from_backend: bool = False
    ) -> list[BuildEntry]:
        build_lst: list[BuildEntry] = []

        _ = await self._lock.acquire()
        try:
            for entry in self._builds.values():
                if owner and entry.user != owner:
                    continue

                if from_backend:
                    entry_copy = entry.model_copy()
                    task_res = CeleryTaskResult(  # pyright: ignore[reportUnknownVariableType]
                        entry.task_id,
                    )
                    entry_copy.state = EntryState(task_res.state)
                    if entry_copy.state == "FAILURE" or entry_copy.state == "SUCCESS":
                        entry_copy.finished = task_res.date_done
                    build_lst.append(entry_copy)
                else:
                    build_lst.append(entry)
        finally:
            self._lock.release()

        return build_lst

    async def abort_build(self, task_id: str, user: str, force: bool) -> None:
        _ = await self._lock.acquire()
        try:
            if task_id not in self._builds:
                raise NoSuchBuildError(task_id)
            entry = self._builds[task_id]

            if not force and entry.user != user:
                raise UnauthorizedTrackerError(user, f"abort build '{task_id}'")

            task = CeleryTaskResult(  # pyright: ignore[reportUnknownVariableType]
                entry.task_id,
            )
            task.revoke(terminate=True, signal="KILL", wait=False)
        finally:
            self._lock.release()
        pass

    async def _mark_task_state(
        self,
        task_id: str,
        state: EntryState,
        *,
        started: dt | None = None,
        finished: dt | None = None,
    ) -> None:
        _ = await self._lock.acquire()
        try:
            entry = self._builds.get(task_id)
            if not entry:
                logger.error(
                    f"unexpected missing task '{task_id}', "
                    + f"can't mark {state.name}!!"
                )
                return

            entry.state = state
            if started:
                entry.started = started
            if finished:
                entry.finished = finished

        finally:
            self._lock.release()

    async def mark_started(self, task_id: str, ts: dt) -> None:
        logger.info(f"task {task_id} started, ts = {ts}")
        await self._mark_task_state(task_id, EntryState.started, started=ts)

    async def mark_succeeded(self, task_id: str, ts: dt) -> None:
        logger.info(f"task {task_id} failed, ts = {ts}")
        await self._mark_task_state(task_id, EntryState.success, finished=ts)

    async def mark_failed(self, task_id: str, ts: dt) -> None:
        logger.info(f"task {task_id} failed, ts = {ts}")
        await self._mark_task_state(task_id, EntryState.failure, finished=ts)

    async def mark_rejected(self, task_id: str) -> None:
        logger.info(f"task {task_id} rejected")
        now = dt.now(tz=datetime.UTC)
        await self._mark_task_state(task_id, EntryState.rejected, finished=now)

    async def mark_revoked(self, task_id: str) -> None:
        logger.info(f"task {task_id} revoked")
        now = dt.now(tz=datetime.UTC)
        await self._mark_task_state(task_id, EntryState.revoked, finished=now)


_builds_tracker = BuildsTracker()


def get_builds_tracker() -> BuildsTracker:
    return _builds_tracker


CBSBuildsTracker = Annotated[BuildsTracker, Depends(get_builds_tracker)]
