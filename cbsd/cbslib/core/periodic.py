# CBS service library - core - periodic tasks
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
import uuid
from collections.abc import Awaitable, Callable
from datetime import datetime as dt

import croniter
import pydantic
from cbsdcore.versions import BuildDescriptor

from cbslib.builds.mgr import BuildsMgr
from cbslib.core import logger as parent_logger

logger = parent_logger.getChild("periodic")


class PeriodicTask(pydantic.BaseModel):
    """Represents a periodic task."""

    cron_format: str
    cron_uuid: uuid.UUID


class PeriodicBuildTask(PeriodicTask):
    """Represents a periodic build task."""

    user: str
    descriptor: BuildDescriptor

    async def trigger(self, mgr: BuildsMgr) -> None:
        logger.info(f"triggering periodic build '{self.cron_uuid}'")
        build_id, build_state = await mgr.new(self.user, self.descriptor)
        logger.info(f"triggered periodic build '{build_id}', state '{build_state}'")


class PeriodicTracker:
    """Keeps track of periodic tasks."""

    _lock: asyncio.Lock
    _tasks: dict[uuid.UUID, asyncio.Task[None]]
    _crons: dict[uuid.UUID, croniter.croniter]
    _builds_mgr: BuildsMgr

    def __init__(self, builds_mgr: BuildsMgr) -> None:
        self._lock = asyncio.Lock()
        self._tasks = {}
        self._crons = {}
        self._builds_mgr = builds_mgr

    async def add_task(self, period: str) -> None:
        try:
            cron = croniter.croniter(period, dt.now(datetime.UTC))
        except croniter.CroniterError as e:
            msg = f"error obtaining cron pattern for period '{period}': {e}"
            logger.error(msg)
            raise ValueError(msg) from e

        cron_uuid = uuid.uuid4()

        async with self._lock:
            self._crons[cron_uuid] = cron

        await self._setup_task(cron_uuid)

    async def _setup_task(self, cron_uuid: uuid.UUID) -> None:
        logger.info(f"setup next task periodic run for '{cron_uuid}'")
        async with self._lock:
            if cron_uuid in self._tasks:
                logger.warning(
                    f"periodic task '{cron_uuid}' is already scheduled, skipping setup"
                )
                return

            if cron_uuid not in self._crons:
                logger.warning(
                    f"periodic task '{cron_uuid}' not found in cron list, "
                    + "skipping setup"
                )
                return

            next_run = self._crons[cron_uuid].get_next(dt)
            logger.info(
                f"setting run time for periodic task '{cron_uuid}' to {next_run}"
            )

            task = asyncio.create_task(
                self._periodic_task_runner(cron_uuid, next_run, self._on_task_finished),
                name=f"periodic-task-runner-{cron_uuid}",
            )
            self._tasks[cron_uuid] = task

    async def _on_task_finished(self, cron_uuid: uuid.UUID) -> None:
        logger.info(f"periodic task '{cron_uuid}' finished")
        async with self._lock:
            del self._tasks[cron_uuid]

        await self._setup_task(cron_uuid)

    async def _periodic_task_runner(
        self,
        cron_uuid: uuid.UUID,
        when: dt,
        on_finished: Callable[[uuid.UUID], Awaitable[None]],
    ) -> None:
        try:
            logger.info(f"runner for periodic task '{cron_uuid}' at {when}")
            now = dt.now(datetime.UTC)
            wait_time = 0 if when <= now else (when - now).total_seconds()
            logger.info(f"periodic task '{cron_uuid}' will run in {wait_time} seconds")
            await asyncio.sleep(wait_time)
        except Exception as e:
            logger.error(f"error in periodic task runner for '{cron_uuid}': {e}")
            return
        await self._periodic_task(cron_uuid, on_finished=on_finished)

    async def _periodic_task(
        self, cron_uuid: uuid.UUID, on_finished: Callable[[uuid.UUID], Awaitable[None]]
    ) -> None:
        logger.info(f"Starting periodic task '{cron_uuid}'")
        await asyncio.sleep(10)
        logger.info(f"Completed periodic task '{cron_uuid}'")
        await on_finished(cron_uuid)
