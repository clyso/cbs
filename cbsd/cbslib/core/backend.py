# CBS service library - core - backend
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


import redis.asyncio as aioredis
from cbscore.errors import CESError

from cbslib.core import logger as parent_logger

logger = parent_logger.getChild("backend")


_CELERY_WORKER_TASKS = [
    "cbslib.worker.tasks",
]


class BackendError(CESError):
    """Base backend error."""

    pass


class Backend:
    _redis_url: str
    # _redis: aioredis.Redis

    def __init__(self, backend_url: str) -> None:
        self._redis_url = backend_url

    async def redis(self) -> aioredis.Redis:
        try:
            return aioredis.from_url(f"{self._redis_url}?decode_responses=True")  # pyright: ignore[reportUnknownMemberType]
        except Exception as e:
            msg = f"error opening connection to redis backend: {e}"
            logger.error(msg)
            raise BackendError(msg) from e
