# CBS - builds - tracker
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
from datetime import datetime as dt
from typing import Annotated, override

from cbslib.builds.types import BuildEntry
from cbslib.worker import tasks
from celery.result import AsyncResult as CeleryTaskResult
from ceslib.errors import CESError
from ceslib.versions.desc import VersionDescriptor
from fastapi import Depends


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
    _builds_by_version: dict[str, BuildEntry]
    _lock: asyncio.Lock

    def __init__(self) -> None:
        self._builds = {}
        self._builds_by_version = {}
        self._lock = asyncio.Lock()

    async def new(self, desc: VersionDescriptor) -> tuple[str, str]:
        _ = await self._lock.acquire()
        try:
            if desc.version in self._builds_by_version:
                raise BuildExistsError(desc.version)

            task = tasks.build.apply_async((desc,), serializer="pydantic")

            build_entry = BuildEntry(
                task_id=task.task_id,
                desc=desc,
                user=desc.signed_off_by.email,
                submitted=dt.now(),
                state=task.state,
                finished=None,
            )
            self._builds[build_entry.task_id] = build_entry
            self._builds_by_version[desc.version] = build_entry

            return (build_entry.task_id, build_entry.state)
        finally:
            self._lock.release()

    async def builds(self, owner: str | None = None) -> list[BuildEntry]:
        build_lst: list[BuildEntry] = []

        _ = await self._lock.acquire()
        try:
            for entry in self._builds.values():
                if not entry.finished:
                    task_res = CeleryTaskResult(  # pyright: ignore[reportUnknownVariableType]
                        entry.task_id,
                    )
                    entry.state = task_res.state
                    if entry.state == "FAILURE" or entry.state == "SUCCESS":
                        entry.finished = task_res.date_done

                if owner and entry.user != owner:
                    continue

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


_builds_tracker = BuildsTracker()


def get_builds_tracker() -> BuildsTracker:
    return _builds_tracker


CBSBuildsTracker = Annotated[BuildsTracker, Depends(get_builds_tracker)]
