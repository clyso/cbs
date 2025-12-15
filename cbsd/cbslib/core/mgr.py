# CBS service library - core - mgr
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
from typing import Annotated, Any, cast

import pydantic
from fastapi import Depends

from cbscore.errors import CESError
from cbsdcore.api.responses import AvailableComponent
from cbsdcore.builds.types import BuildEntry
from cbsdcore.versions import BuildDescriptor
from cbslib.builds import logger as parent_logger
from cbslib.builds.db import BuildsDB
from cbslib.builds.tracker import BuildsTracker
from cbslib.config.config import get_config
from cbslib.core.permissions import Permissions
from cbslib.worker.celery import celery_app
from cbslib.worker.tasks import ListComponentsTaskResponse

logger = parent_logger.getChild("mgr")


class MgrError(CESError):
    pass


class NotAvailableError(MgrError):
    pass


class UnknownComponentsError(MgrError):
    components: list[str]

    def __init__(self, unknown_components: list[str]) -> None:
        self.components = unknown_components
        super().__init__()


class Mgr:
    """
    Manages builds, tracking existing builds, etc.

    This is where logic for permissions, version naming conventions, etc., should live.
    """

    _db: BuildsDB
    _permissions: Permissions
    _tracker: BuildsTracker
    _available_components: dict[str, AvailableComponent]
    _started: bool
    _init_task: asyncio.Task[None] | None

    def __init__(self, db_path: Path, permissions_path: Path) -> None:
        self._db = BuildsDB(db_path)

        try:
            self._permissions = Permissions.load(permissions_path)
        except (ValueError, CESError) as e:
            msg = f"failed to load permissions from '{permissions_path}': {e}"
            logger.error(msg)
            raise MgrError(msg) from e

        logger.info(
            "loaded permissions: "
            + f"{len(self._permissions.groups)} groups, "
            + f"{len(self._permissions.rules)} rules"
        )

        self._tracker = BuildsTracker()
        self._available_components = {}
        self._started = False
        self._init_task = None

        self._update_components()

    def _update_components(self) -> None:
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

        loop = asyncio.get_running_loop()
        self._init_task = loop.create_task(_task())

    async def new(self, desc: BuildDescriptor) -> tuple[str, str]:
        """Start a new build."""
        if not self._started:
            logger.warning("service not started yet, try again later")
            raise NotAvailableError()

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

    async def revoke(self, build_id: str, user: str, force: bool) -> None:
        """Revoke a given build."""
        if not self._started:
            logger.warning("service not started yet, try again later")
            raise NotAvailableError()

        # propagate exceptions
        await self._tracker.revoke(build_id, user, force)

    async def status(
        self, *, owner: str | None = None, from_backend: bool = False
    ) -> list[BuildEntry]:
        """List known builds."""
        if not self._started:
            logger.warning("service not started yet, try again later")
            raise NotAvailableError()

        # propagate exceptions
        return await self._tracker.list(owner=owner, from_backend=from_backend)

    @property
    def components(self) -> dict[str, AvailableComponent]:
        """Obtain known components list."""
        if not self._started:
            raise NotAvailableError()
        return self._available_components

    @property
    def tracker(self) -> BuildsTracker:
        return self._tracker


_mgr: Mgr | None = None


def mgr_init() -> Mgr:
    logger.info("init cbs service mgr")
    global _mgr

    if not _mgr:
        config = get_config()
        assert config.server, "unexpected missing server config"
        _mgr = Mgr(config.server.db, config.server.permissions)

    return _mgr


def get_mgr() -> Mgr:
    assert _mgr, "CBS service manager not set up"
    return _mgr


CBSMgr = Annotated[Mgr, Depends(get_mgr)]
