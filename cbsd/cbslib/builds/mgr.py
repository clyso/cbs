# CBS service library - builds - build manager
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
import errno
import sys
from pathlib import Path
from typing import Any, cast

import pydantic
from cbscore.errors import CESError
from cbsdcore.api.responses import AvailableComponent
from cbsdcore.builds.types import BuildEntry, BuildID
from cbsdcore.versions import BuildDescriptor

from cbslib.builds import logger as parent_logger
from cbslib.builds.db import BuildsDB
from cbslib.builds.tracker import BuildsTracker
from cbslib.core.permissions import AuthorizationCaps, NotAuthorizedError, Permissions
from cbslib.worker.celery import celery_app
from cbslib.worker.tasks import ListComponentsTaskResponse

logger = parent_logger.getChild("mgr")


class BuildsMgrError(CESError):
    pass


class NotAvailableError(BuildsMgrError):
    """Service currently not available."""

    pass


class UnknownComponentsError(BuildsMgrError):
    """Unknown components have been specified."""

    components: list[str]

    def __init__(self, unknown_components: list[str]) -> None:
        super().__init__()
        self.components = unknown_components


def _check_new_descriptor_permissions(
    user: str, permissions: Permissions, desc: BuildDescriptor
) -> bool:
    """Validate whether a given user is authorized for a new build."""
    logger.warning(f"check new build permissions for user '{user}'")
    if desc.channel.startswith("!"):
        # channel variables not implemented yet, maybe soon-ish. These are
        # meant to allow having user channels, group channels, etc.
        logger.warning(f"user '{user}' build refused for own channel")
        return False

    if not permissions.is_authorized_for_project(
        user, desc.channel, AuthorizationCaps.BUILDS_CREATE
    ):
        logger.warning(f"user '{user}' build refused for channel '{desc.channel}'")
        return False

    for comp in desc.components:
        if comp.repo and not permissions.is_authorized_for_repository(user, comp.repo):
            logger.warning(f"user '{user}' build refused for repository '{comp.repo}'")
            return False

    return True


class BuildsMgr:
    """Manages build-related operations."""

    _db: BuildsDB
    _permissions: Permissions
    _tracker: BuildsTracker
    _available_components: dict[str, AvailableComponent]
    _started: bool
    _init_task: asyncio.Task[None] | None

    def __init__(self, db_path: Path, permissions: Permissions) -> None:
        self._db = BuildsDB(db_path)
        self._permissions = permissions
        self._tracker = BuildsTracker(self._db)
        self._available_components = {}
        self._started = False
        self._init_task = None

    async def init(self) -> None:
        """Perform initialisation tasks."""
        # garbage collect old unfinished builds that may have lingered if we were
        # hard shutdown.
        await self._db.gc()

        # update our known components.
        await self._update_components()

    async def _update_components(self) -> None:
        """Update components list, before we can start servicing requests."""
        # this function could be run regularly in the background.
        # we need to take into account that, in that case, will be scheduled
        # alongside other tasks, and will have to wait for tasks to finish
        # before being able to run -- unless we do multiple queues.
        logger.info("update mgr available components")

        async def _task() -> None:
            try:
                res = celery_app.send_task("cbslib.worker.tasks.list_components")
                raw = cast(dict[str, Any], res.get())  # pyright: ignore[reportExplicitAny]
            except Exception as e:
                logger.error(f"failed to obtain components: {e}")
                sys.exit(errno.ENOTRECOVERABLE)

            logger.info(f"obtained components list from worker: {raw}")

            try:
                comp_res = ListComponentsTaskResponse.model_validate(raw)
            except pydantic.ValidationError as e:
                logger.error(f"failed to validate response: {e}")
                sys.exit(errno.EINVAL)

            self._available_components = comp_res.components
            self._started = True
            self._init_task = None
            logger.info("mgr now available")

        self._init_task = asyncio.create_task(_task())

    async def new(self, user: str, desc: BuildDescriptor) -> tuple[BuildID, str]:
        """Start a new build."""
        if not self._started:
            logger.warning("service not started yet, try again later")
            raise NotAvailableError()

        if not _check_new_descriptor_permissions(user, self._permissions, desc):
            raise NotAuthorizedError()

        unknown_components = [
            c.name for c in desc.components if c.name not in self._available_components
        ]
        if unknown_components:
            logger.warning(
                f"unknown components for build request: {unknown_components}"
            )
            raise UnknownComponentsError(unknown_components)

        # propagate exceptions
        return await self._tracker.new(desc)

    async def revoke(self, build_id: BuildID, user: str, force: bool) -> None:
        """Revoke a given build."""
        if not self._started:
            logger.warning("service not started yet, try again later")
            raise NotAvailableError()

        # propagate exceptions
        await self._tracker.revoke(build_id, user, force)

    async def status(
        self, *, owner: str | None = None
    ) -> list[tuple[BuildID, BuildEntry]]:
        """List known builds."""
        if not self._started:
            logger.warning("service not started yet, try again later")
            raise NotAvailableError()

        # propagate exceptions
        return await self._tracker.list(owner=owner)

    @property
    def available(self) -> bool:
        return self._started

    @property
    def components(self) -> dict[str, AvailableComponent]:
        """Obtain known components list."""
        if not self._started:
            raise NotAvailableError()
        return self._available_components

    @property
    def tracker(self) -> BuildsTracker:
        return self._tracker
