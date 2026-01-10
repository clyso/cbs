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
from typing import override

from cbscore.errors import CESError
from cbsdcore.builds.types import BuildEntry, BuildID, EntryState
from cbsdcore.versions import BuildDescriptor
from celery.result import AsyncResult as CeleryTaskResult

from cbslib.builds import logger as parent_logger
from cbslib.builds.db import BuildsDB, BuildsDBError
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
    """Tracks existing builds, tracking them as they are sent to workers."""

    _db: BuildsDB
    _builds_by_task_id: dict[str, BuildID]
    _builds_by_build_id: dict[BuildID, str]
    _lock: asyncio.Lock

    def __init__(self, db: BuildsDB) -> None:
        self._db = db
        self._builds_by_task_id = {}
        self._builds_by_build_id = {}
        self._lock = asyncio.Lock()

    async def new(self, desc: BuildDescriptor) -> tuple[BuildID, str]:
        """Create a new build entry, scheduling it for build."""
        _ = await self._lock.acquire()
        try:
            # NOTE: We should ensure builds that are the same are properly
            # deduplicated if they are in-progress. I.e., if the same build
            # request comes twice, and it's either in-progress or queued, then
            # we return its build ID.

            build_entry = BuildEntry(
                task_id=None,
                desc=desc,
                user=desc.signed_off_by.email,
                submitted=dt.now(tz=datetime.UTC),
                state=EntryState.new,
                started=None,
                finished=None,
            )
            build_id = await self._db.new(build_entry)

            # schedule version for building
            task = tasks.build.apply_async(
                (
                    build_id,
                    desc,
                ),
                serializer="pydantic",
            )

            build_entry.task_id = task.task_id
            build_entry.state = EntryState(task.state.upper())
            await self._db.update(build_id, build_entry)

            self._builds_by_task_id[build_entry.task_id] = build_id
            self._builds_by_build_id[build_id] = build_entry.task_id

        except Exception as e:
            msg = f"error scheduling new build: {e}"
            logger.error(msg)
            raise TrackerError(msg) from e
        else:
            return (build_id, build_entry.state)
        finally:
            self._lock.release()

    async def list(
        self, *, owner: str | None = None
    ) -> list[tuple[BuildID, BuildEntry]]:
        """List all known builds, from stable storage, optionally filtering by owner."""
        try:
            db_builds = await self._db.ls()
        except BuildsDBError as e:
            logger.warning(f"failed to list builds from db: {e}")
            raise TrackerError(f"failed to list builds: {e}") from e

        builds: list[tuple[BuildID, BuildEntry]] = []
        for db_entry in db_builds:
            entry = db_entry.entry
            if owner is None or entry.user == owner:
                builds.append((db_entry.build_id, entry))

        return builds

    async def revoke(self, build_id: BuildID, user: str, force: bool) -> None:
        """
        Revoke an on-going build.

        Does not persist task state in the database, only triggers the revoke on the
        task itself, on the worker.

        Persistent state will only be updated when the worker reports back through
        events.
        """
        async with self._lock:
            if build_id not in self._builds_by_build_id:
                raise NoSuchBuildError()

            task_id = self._builds_by_build_id[build_id]
            if task_id not in self._builds_by_task_id:
                logger.error(
                    f"unexpected missing task '{task_id}' for build '{build_id}'"
                )
                raise NoSuchBuildError()

            try:
                db_entry = await self._db.get(build_id)
            except BuildsDBError as e:
                msg = f"failed to get build '{build_id}' from db: {e}"
                logger.error(msg)
                raise TrackerError(msg) from e

            if not force and user != db_entry.entry.user:
                raise UnauthorizedTrackerError(user, f"revoke build '{build_id}'")

            entry = db_entry.entry
            if not entry.task_id:
                if entry.state != EntryState.new:
                    msg = (
                        "unexpected missing task id for "
                        + f"non-new task (state '{entry.state.value}')"
                    )
                    logger.error(msg)
                    raise TrackerError(msg)
                else:
                    raise NoSuchBuildError(f"build '{build_id}' yet to be scheduled")

            try:
                task = CeleryTaskResult(  # pyright: ignore[reportUnknownVariableType]
                    entry.task_id,
                )
                task.revoke(terminate=True, signal="KILL", wait=False)
            except Exception as e:
                msg = f"failed to revoke build '{build_id}': {e}"
                logger.error(msg)
                raise TrackerError(msg) from e

    async def _mark_task_state(
        self,
        task_id: str,
        state: EntryState,
        *,
        started: dt | None = None,
        finished: dt | None = None,
    ) -> None:
        async with self._lock:
            build_id = self._builds_by_task_id.get(task_id)
            if not build_id:
                # not our task, likely not a build, ignore.
                return

            try:
                db_entry = await self._db.get(build_id)
            except BuildsDBError as e:
                msg = f"fialed to get build '{build_id}' from db: {e}"
                logger.warning(msg)
                raise TrackerError(msg) from e

            entry = db_entry.entry
            entry.state = state

            if started:
                entry.started = started
            if finished:
                entry.finished = finished

            await self._db.update(build_id, entry)

            if entry.state in [
                EntryState.success,
                EntryState.failure,
                EntryState.revoked,
                EntryState.rejected,
            ]:
                logger.debug(
                    f"removing completed build tracking for task {task_id}, "
                    + f"state '{entry.state}'"
                )
                del self._builds_by_task_id[task_id]
                del self._builds_by_build_id[build_id]

    async def mark_started(self, task_id: str, ts: dt) -> None:
        logger.info(f"task {task_id} started, ts = {ts}")
        await self._mark_task_state(task_id, EntryState.started, started=ts)

    async def mark_succeeded(self, task_id: str, ts: dt) -> None:
        logger.info(f"task {task_id} succeeded, ts = {ts}")
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
