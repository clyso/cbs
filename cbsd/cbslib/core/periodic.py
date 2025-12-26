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
from cbscore.errors import CESError
from cbsdcore.versions import BuildDescriptor

from cbslib.builds.mgr import BuildsMgr, NotAvailableError
from cbslib.core import logger as parent_logger
from cbslib.core.utils import format_to_str

logger = parent_logger.getChild("periodic")


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

    _lock: asyncio.Lock
    _tasks: dict[uuid.UUID, asyncio.Task[None]]
    _crons: dict[uuid.UUID, croniter.croniter]
    _builds_mgr: BuildsMgr

    # NOTE: this could be a map of 'PeriodicTask' instead, as having it as a base class
    # could allow us to have multiple types of periodic tasks -- simply have them all
    # implement an override to a 'trigger' abstract method. Keep it as future work,
    # if/when we need other types of periodic tasks.
    #
    _tasks_descs: dict[uuid.UUID, PeriodicBuildTask]

    def __init__(self, builds_mgr: BuildsMgr) -> None:
        self._lock = asyncio.Lock()
        self._tasks = {}
        self._crons = {}
        self._builds_mgr = builds_mgr
        self._tasks_descs = {}

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

        try:
            cron = croniter.croniter(cron_format, dt.now(datetime.UTC))
        except (
            croniter.CroniterBadCronError,
            croniter.CroniterNotAlphaError,
            croniter.CroniterUnsupportedSyntaxError,
        ) as e:
            logger.warning(f"potentially bad cron format '{cron_format}': {e}")
            raise BadCronFormatError(cron_format) from e
        except croniter.CroniterError as e:
            msg = f"error parsing cron pattern for '{cron_format}': {e}"
            logger.error(msg)
            raise PeriodicTrackerError(msg) from e

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

        async with self._lock:
            self._crons[cron_uuid] = cron
            self._tasks_descs[cron_uuid] = periodic_task

        await self._setup_task(cron_uuid)

        return cron_uuid

    async def _setup_task(self, cron_uuid: uuid.UUID) -> None:
        """Set up a task to be periodically run."""
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
        async with self._lock:
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
