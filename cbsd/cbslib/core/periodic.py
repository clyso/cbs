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
import dbm.sqlite3 as sqlite3
import uuid
from collections.abc import Awaitable, Callable
from datetime import datetime as dt
from pathlib import Path

import aiorwlock
import croniter
import pydantic
from cbscore.errors import CESError
from cbsdcore.versions import BuildDescriptor

from cbslib.builds.mgr import BuildsMgr, NotAvailableError
from cbslib.core import logger as parent_logger
from cbslib.core.utils import format_to_str

logger = parent_logger.getChild("periodic")

_PERIODIC_TASKS_DB_FILE = "periodic_tasks.db"


class PeriodicTrackerError(CESError):
    """Base Periodic Tracker error."""

    pass


class BadCronFormatError(PeriodicTrackerError):
    """Provided cron format is not correct or may generate incorrect values."""

    def __init__(self, cron_format: str) -> None:
        super().__init__(
            f"cron format not correct or may generate incorrect values: {cron_format}"
        )


class TryAgainError(PeriodicTrackerError):
    """Must try again at a later time."""

    pass


class DisableTaskError(PeriodicTrackerError):
    """Must disable the task."""

    pass


class NoSuchTaskError(PeriodicTrackerError):
    """Task does not exist."""

    def __init__(self, cron_uuid: uuid.UUID) -> None:
        super().__init__(f"task uuid '{cron_uuid}' does not exist")


def _format_tag_to_str(tag_format: str, desc: BuildDescriptor) -> str:
    return format_to_str(
        tag_format,
        {
            "version": desc.version,
            "base_tag": desc.dst_image.tag,
            "channel": desc.channel,
            "user": desc.signed_off_by.user,
            "arch": desc.build.arch,
            "distro": desc.build.distro,
            "os_version": desc.build.os_version,
        },
    )


class PeriodicTask(pydantic.BaseModel):
    """Represents a periodic task."""

    cron_format: str
    cron_uuid: uuid.UUID
    enabled: bool

    created_by_user: str
    summary: str | None = pydantic.Field(default=None)


class PeriodicBuildTask(PeriodicTask):
    """Represents a periodic build task."""

    descriptor: BuildDescriptor
    tag_format: str

    async def trigger(self, mgr: BuildsMgr) -> None:
        logger.info(f"triggering periodic build '{self.cron_uuid}'")

        new_descriptor = self.descriptor.model_copy(deep=True)
        new_descriptor.dst_image.tag = self.formatted_tag

        try:
            build_id, build_state = await mgr.new(self.created_by_user, self.descriptor)
        except NotAvailableError:
            logger.warning("unable to build at this time, backoff and try again")
            raise TryAgainError() from None
        except Exception as e:
            logger.error(f"error running periodic build: {e}")
            raise DisableTaskError() from None

        logger.info(f"triggered periodic build '{build_id}', state '{build_state}'")

    @property
    def formatted_tag(self) -> str:
        return _format_tag_to_str(self.tag_format, self.descriptor)


class PeriodicTracker:
    """Keeps track of periodic tasks."""

    _db_file_path: Path

    # use aiorwlock instead of asyncio.Lock because the latter is not reentrant, and as
    # it is, that's exceptionally useful for us right now.
    _lock: aiorwlock.RWLock
    _tasks: dict[uuid.UUID, asyncio.Task[None]]
    _crons: dict[uuid.UUID, croniter.croniter]
    _builds_mgr: BuildsMgr

    # NOTE: this could be a map of 'PeriodicTask' instead, as having it as a base class
    # could allow us to have multiple types of periodic tasks -- simply have them all
    # implement an override to a 'trigger' abstract method. Keep it as future work,
    # if/when we need other types of periodic tasks.
    #
    _tasks_descs: dict[uuid.UUID, PeriodicBuildTask]

    def __init__(self, builds_mgr: BuildsMgr, db_path: Path) -> None:
        self._lock = aiorwlock.RWLock()
        self._tasks = {}
        self._crons = {}
        self._builds_mgr = builds_mgr
        self._tasks_descs = {}

        db_path.mkdir(parents=True, exist_ok=True)
        if not db_path.is_dir():
            msg = "database path is not a directory"
            logger.error(msg)
            raise PeriodicTrackerError(msg)

        self._db_file_path = db_path / _PERIODIC_TASKS_DB_FILE

    async def init(self) -> None:
        """Initialize the periodic tracker, loading tasks from disk."""
        try:
            await self._load()
        except Exception as e:
            msg = f"failed to load periodic tasks database: {e}"
            logger.error(msg)
            raise PeriodicTrackerError(msg) from e

    async def add_build_task(
        self,
        cron_format: str,
        tag_format: str,
        created_by: str,
        descriptor: BuildDescriptor,
        summary: str | None = None,
    ) -> uuid.UUID:
        """
        Add a new periodic build task.

        Takes a `cron_format` describing the period for execution following the crontab
        format.

        Takes a `tag_format` that will define the built images tags. A `tag_format`
        should include python string templates that are recognized by the
        `_format_tag_to_str()` function.

        Takes a `created_by`, specifying the user that is creating this periodic task.

        Takes a `descriptor` defining the build to be created.
        """
        if not cron_format:
            raise PeriodicTrackerError("cron format not provided")

        if not tag_format:
            raise PeriodicTrackerError("tag format not provided")

        if not created_by:
            raise PeriodicTrackerError("creating user not provided")

        cron_uuid = uuid.uuid4()

        periodic_task = PeriodicBuildTask(
            cron_format=cron_format,
            cron_uuid=cron_uuid,
            enabled=True,
            created_by_user=created_by,
            summary=summary,
            descriptor=descriptor,
            tag_format=tag_format,
        )

        # propagate exceptions going forward, let the caller handle them.
        res = await self._add_periodic_task(cron_uuid, periodic_task)
        await self._save_task(periodic_task)

        return res

    async def _add_periodic_task(
        self, cron_uuid: uuid.UUID, periodic_task: PeriodicBuildTask
    ) -> uuid.UUID:
        """
        Add a periodic task to the tracker.

        This is a helper function to deduplicate code between different code paths
        adding a task to the tracker (e.g., 'add_build_task()' and loading from disk).
        """
        try:
            cron = croniter.croniter(periodic_task.cron_format, dt.now(datetime.UTC))
        except (
            croniter.CroniterBadCronError,
            croniter.CroniterNotAlphaError,
            croniter.CroniterUnsupportedSyntaxError,
        ) as e:
            logger.warning(
                f"potentially bad cron format '{periodic_task.cron_format}': {e}"
            )
            raise BadCronFormatError(periodic_task.cron_format) from e
        except croniter.CroniterError as e:
            msg = f"error parsing cron pattern for '{periodic_task.cron_format}': {e}"
            logger.error(msg)
            raise PeriodicTrackerError(msg) from e

        async with self._lock.writer_lock:
            self._crons[cron_uuid] = cron
            self._tasks_descs[cron_uuid] = periodic_task

        await self._setup_task(cron_uuid)

        return cron_uuid

    async def _setup_task(self, cron_uuid: uuid.UUID) -> None:
        """Set up a task to be periodically run."""
        logger.info(f"setup next task periodic run for '{cron_uuid}'")
        async with self._lock.writer_lock:
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

            if cron_uuid not in self._tasks_descs:
                logger.warning(
                    f"periodic build task for cron '{cron_uuid}' not found, "
                    + "skipping setup."
                )
                return

            task_desc = self._tasks_descs[cron_uuid]
            if not task_desc.enabled:
                logger.warning(f"task '{cron_uuid}' not enabled, skipping setup.")
                return

            next_run = self._crons[cron_uuid].get_next(dt)
            logger.info(
                f"setting run time for periodic task '{cron_uuid}' to {next_run}"
            )

            task = asyncio.create_task(
                self._periodic_task_runner(
                    cron_uuid,
                    next_run,
                    task_desc,
                    self._on_task_finished,
                ),
                name=f"periodic-task-runner-{cron_uuid}",
            )
            self._tasks[cron_uuid] = task

    async def _on_task_finished(
        self, cron_uuid: uuid.UUID, disable: bool = False
    ) -> None:
        """
        Define a callback for when a task is finished.

        We need this to be a callback set up from `_setup_task()` so we can ensure that
        it is executed in the same event loop as the original `_setup_task()` call from
        when we first added the task -- which is where the this class' lock lives.

        Given we need to acquire the lock, we need for it to be run from the same event
        loop. If we were trying to acquire the lock from the finalized running task, we
        would have to be doing alternative foos (like passing the event loop to the
        task, etc.).

        If `disable` is set to True on calling back, then we will mark the task as
        not enabled before we run the next `_setup_task()`.
        """
        logger.info(f"periodic task '{cron_uuid}' finished")
        async with self._lock.writer_lock:
            del self._tasks[cron_uuid]

            if disable and (desc := self._tasks_descs.get(cron_uuid, None)):
                desc.enabled = False

        await self._setup_task(cron_uuid)

    async def _periodic_task_runner(
        self,
        cron_uuid: uuid.UUID,
        when: dt,
        periodic_task: PeriodicBuildTask,
        on_finished: Callable[[uuid.UUID, bool], Awaitable[None]],
    ) -> None:
        """
        Run a periodic task at the specified datetime.

        This function should always be called as a task, with an `on_finished` callback
        originating on the class' original event loop.
        """
        backoff = 30.0  # default backoff seconds
        max_backoff = 60.0 * 10  # max backoff 10 minutes
        backoff_factor = 1.5

        while backoff < max_backoff:
            try:
                logger.info(f"runner for periodic task '{cron_uuid}' at {when}")
                now = dt.now(datetime.UTC)
                wait_time = 0 if when <= now else (when - now).total_seconds()
                logger.info(
                    f"periodic task '{cron_uuid}' will run in {wait_time} seconds"
                )
                await asyncio.sleep(wait_time)
            except Exception as e:
                logger.error(f"error in periodic task runner for '{cron_uuid}': {e}")
                return

            try:
                await periodic_task.trigger(self._builds_mgr)

            except TryAgainError:
                logger.warning(
                    f"must backoff executing '{cron_uuid}', backoff '{backoff}' seconds"
                )
                backoff *= backoff_factor
                continue

            except DisableTaskError:
                logger.warning(f"task disable requested for '{cron_uuid}'")
                await on_finished(cron_uuid, True)

            except Exception as e:
                logger.error(f"unexpected error triggering '{cron_uuid}': {e}")
                logger.warning(f"disabling '{cron_uuid}'")
                await on_finished(cron_uuid, True)

            await on_finished(cron_uuid, False)
            # return here so we can handle the expired backoff when the while
            # condition fails.
            return

        logger.warning(
            f"max backoff of {backoff} seconds reached for '{cron_uuid}', disable task."
        )
        await on_finished(cron_uuid, True)

    async def ls(self) -> list[tuple[dt | None, PeriodicBuildTask]]:
        """
        List all known periodic tasks.

        Returns a list of tuples, each containing the next run for the task,
        and the task's tracker entry.
        """
        known_tasks: list[tuple[dt | None, PeriodicBuildTask]] = []
        async with self._lock.reader_lock:
            for cron_uuid, entry in self._tasks_descs.items():
                next_run: dt | None = None
                if entry.enabled:
                    if cron_uuid not in self._crons:
                        logger.error(f"missing cron for '{cron_uuid}'!! skipping.")
                        continue
                    next_run = self._crons[cron_uuid].get_current(dt)

                known_tasks.append((next_run, entry.model_copy(deep=True)))

        return known_tasks

    async def disable(self, cron_uuid: uuid.UUID) -> None:
        """Disable a given task, if it exists."""
        logger.info(f"received request to disable '{cron_uuid}'")
        async with self._lock.writer_lock:
            if cron_uuid not in self._tasks_descs:
                raise NoSuchTaskError(cron_uuid)

            if not self._tasks_descs[cron_uuid].enabled:
                if cron_uuid in self._crons:
                    # just some sanity checking, this should never happen.
                    logger.error(f"disabled task '{cron_uuid}' has an active cron!!!")
                return

            if cron_uuid not in self._crons:
                logger.error(f"unexpected missing cron for task '{cron_uuid}'!!!")
                return

            del self._crons[cron_uuid]

            if cron_uuid in self._tasks:
                _ = self._tasks[cron_uuid].cancel()
                del self._tasks[cron_uuid]

            self._tasks_descs[cron_uuid].enabled = False
            await self._save_task(self._tasks_descs[cron_uuid])

        logger.info(f"disabled task '{cron_uuid}'")

    async def _load(self) -> None:
        """Load the database of periodic tasks from disk."""
        logger.info("loading periodic tasks database from disk")
        async with self._lock.writer_lock:
            try:
                with sqlite3.open(self._db_file_path, "c") as db:
                    for cron_uuid_key in db:
                        task_data = db[cron_uuid_key]
                        task_desc = PeriodicBuildTask.model_validate_json(task_data)

                        try:
                            _ = await self._add_periodic_task(
                                cron_uuid=task_desc.cron_uuid, periodic_task=task_desc
                            )
                        except BadCronFormatError as e:
                            logger.error(
                                f"error parsing cron format for task from db: {e} "
                                + "-- ignore task."
                            )

            except pydantic.ValidationError as e:
                msg = f"error loading periodic task from db:\n{e}"
                logger.error(msg)
                raise PeriodicTrackerError(msg) from e

            except PeriodicTrackerError as e:
                raise e from e

            except Exception as e:
                msg = f"error loading periodic tasks from db: {e}"
                logger.error(msg)
                raise PeriodicTrackerError(msg) from e

            logger.info(f"loaded {len(self._tasks_descs)} periodic tasks from database")

    async def _save_task(self, periodic_task: PeriodicBuildTask) -> None:
        """Store a periodic task to the database on disk."""
        logger.info(f"saving periodic task '{periodic_task.cron_uuid}' to disk")

        db_key = periodic_task.cron_uuid.bytes
        db_data = periodic_task.model_dump_json()

        # acquire lock to ensure no other operation is modifying the database as
        # we write to it.
        async with self._lock.writer_lock:
            try:
                with sqlite3.open(self._db_file_path, "c") as db:
                    db[db_key] = db_data
            except Exception as e:
                msg = (
                    f"error saving periodic task '{periodic_task.cron_uuid}' to db: {e}"
                )
                logger.error(msg)
                raise PeriodicTrackerError(msg) from e

        logger.info(f"saved periodic task '{periodic_task.cron_uuid}' to disk")
