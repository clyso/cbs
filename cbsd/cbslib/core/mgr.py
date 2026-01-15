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


from typing import Annotated

from cbscore.errors import CESError
from fastapi import Depends

from cbslib.builds import logger as parent_logger
from cbslib.builds.mgr import BuildsMgr
from cbslib.config.config import get_config
from cbslib.config.server import ServerConfig
from cbslib.core.backend import Backend
from cbslib.core.periodic import PeriodicTracker, PeriodicTrackerError
from cbslib.core.permissions import Permissions

logger = parent_logger.getChild("mgr")


class MgrError(CESError):
    pass


class Mgr:
    """
    Manages builds, tracking existing builds, etc.

    This is where logic for permissions, version naming conventions, etc., should live.
    """

    _permissions: Permissions
    _backend: Backend
    _builds_mgr: BuildsMgr
    _periodic_tracker: PeriodicTracker

    def __init__(self, config: ServerConfig, backend_url: str) -> None:
        db_path = config.db
        logs_path = config.logs
        permissions_path = config.permissions

        if not db_path.exists():
            db_path.mkdir(parents=True)

        if not db_path.is_dir():
            msg = "database path is not a directory"
            logger.error(msg)
            raise MgrError(msg)

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

        self._backend = Backend(backend_url)
        self._builds_mgr = BuildsMgr(
            db_path, logs_path, self._permissions, self._backend
        )
        self._periodic_tracker = PeriodicTracker(self._builds_mgr, db_path)

    async def init(self) -> None:
        """Perform operations on the mgr that are required for its proper start."""
        # init builds manager
        await self._builds_mgr.init()

        # set up periodic tasks from db.
        try:
            await self._periodic_tracker.init()
        except PeriodicTrackerError as e:
            msg = f"error initiating core mgr: {e}"
            logger.error(msg)
            raise MgrError(msg) from e

    @property
    def builds_mgr(self) -> BuildsMgr:
        """Return the builds mgr instance, if it is already available."""
        return self._builds_mgr

    @property
    def permissions(self) -> Permissions:
        return self._permissions

    @property
    def periodic_tracker(self) -> PeriodicTracker:
        """Returns the periodic tasks tracker instance."""
        return self._periodic_tracker


_mgr: Mgr | None = None


def mgr_init() -> Mgr:
    logger.info("init cbs service mgr")
    global _mgr

    if not _mgr:
        config = get_config()
        assert config.server, "unexpected missing server config"
        _mgr = Mgr(config.server, config.redis_backend_url)

    return _mgr


def get_mgr() -> Mgr:
    assert _mgr, "CBS service manager not set up"
    return _mgr


CBSMgr = Annotated[Mgr, Depends(get_mgr)]
