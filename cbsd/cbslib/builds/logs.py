# CBS server library - builds - logs handler
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
import logging
import stat
from asyncio.tasks import Task
from pathlib import Path
from typing import Any, cast

import aiofiles
from cbscore.errors import CESError
from cbsdcore.builds.types import BuildID

from cbslib.builds import logger as parent_logger
from cbslib.core.backend import Backend

logger = parent_logger.getChild("logs")
# by default, make sure we're not too verbose. This can be adjusted
# if needed by calling the logger's name on server init.
logger.setLevel(logging.INFO)


_BUILD_LOGS_DIR = "builds"


class BuildLogsHandlerError(CESError):
    """Generic build logs handler error."""

    pass


class BuildLogsHandler:
    """Handle logs for all builds."""

    _logs_path: Path
    _backend: Backend
    _tasks: dict[BuildID, Task[None]]
    _lock: asyncio.Lock

    def __init__(self, logs_path: Path, backend: Backend) -> None:
        """
        Handle logs for all builds, capturing from redis.

        As the worker produces build logs and pushes them to the build's corresponding
        redis stream, we will consume those using build-specific tasks. These will be
        written to disk as they are produced.

        :param Path logs_path: the base path for log files (e.g., /cbs/logs)
        :param Backend backend: the backend to be used for redis connections
        :raises BuildLogsHandlerError: if the logs path exists and is not a directory
        :raises BuildLogsHandlerError: if the log file is not readable/writeable
        """
        self._logs_path = logs_path / _BUILD_LOGS_DIR
        self._backend = backend
        self._tasks = {}
        self._lock = asyncio.Lock()

        if self._logs_path.exists() and not self._logs_path.is_dir():
            msg = f"logs path at '{self._logs_path} exist but not a directory"
            logger.error(msg)
            raise BuildLogsHandlerError(msg)

        if self._logs_path.exists() and not self._logs_path.lstat().st_mode & (
            stat.S_IWRITE | stat.S_IREAD
        ):
            msg = f"logs path at '{self._logs_path} is not read/writable"
            logger.error(msg)
            raise BuildLogsHandlerError(msg)

        self._logs_path.mkdir(parents=True, exist_ok=True)

    async def new(self, build_id: BuildID) -> None:
        """
        Start log gathering for a new build.

        Will create a background task to handle incoming log messages for a given build,
        keeping track of said task. Will require a build to be marked as finished to
        free up resources.

        :param BuildID build_id: The ID for the build being started
        """
        logger.info(f"tracking logs for build '{build_id}'")
        async with self._lock:
            if build_id in self._tasks:
                logger.warning(
                    f"build '{build_id}' already being tracked by log handler"
                )
                return

            self._tasks[build_id] = asyncio.create_task(
                self._logger(build_id), name=f"log-handler-build:{build_id}"
            )

    async def finish(self, build_id: BuildID) -> None:
        """
        Finishes a log gathering task for a given build.

        Will cancel the background task responsible for gathering the logs for the
        specified build, cleaning up as necessary.

        :param BuildID build_id: The ID for the build being finished
        """
        logger.info(f"finishing tracking logs for build '{build_id}'")
        async with self._lock:
            if build_id not in self._tasks:
                logger.warning(f"build '{build_id}' not being tracked for logs")
                return

            try:
                _ = self._tasks[build_id].cancel()
                await self._tasks[build_id]
            except asyncio.CancelledError:
                pass
            except Exception as e:
                logger.error(
                    f"error canceling log tracking task for build '{build_id}': {e}"
                )
                return

            del self._tasks[build_id]

            # TODO: Maybe get reason from arguments, like 'revoked', 'finished', etc.
            log_file_path = self.get_log_path_for(build_id)
            async with aiofiles.open(log_file_path, "+a") as fd:
                _ = await fd.write("--- build log end ---\n")
                await fd.flush()

    def get_log_path_for(self, build_id: BuildID) -> Path:
        """Obtain the log file path for a given build."""
        return self._logs_path / f"build-{build_id}.log"

    async def _logger(self, build_id: BuildID) -> None:
        """
        Handle log messages for a given build, writing them to disk.

        Log messages will be consumed from a redis stream, as messages are added to the
        stream by the worker.

        :param BuildID build_id: The ID for the build being handled.
        """
        build_log_path = self.get_log_path_for(build_id)
        build_stream = f"cbs:logs:builds:{build_id}"
        async with aiofiles.open(build_log_path, "+a") as fd:
            redis = await self._backend.redis()
            while True:
                logger.debug(f"reading from redis stream '{build_stream}'")
                res = await redis.xread(  # pyright: ignore[reportAny]
                    streams={build_stream: "$"}, count=None, block=0
                )
                if not isinstance(res, list):
                    logger.warning(f"got erroneous result from redis: {res} -- stop")
                    return

                # format coming out of res is something like:
                # res = [['cbs:logs:builds:123', [('1768401181174-0', {'msg': '\n'})]]]

                res = cast(
                    list[list[str | list[tuple[str, dict[str, Any]]]]],  # pyright: ignore[reportExplicitAny]
                    res,
                )
                logger.debug(f"from redis stream '{build_stream}': {res}")
                for stream_lst in res:
                    logger.debug(f"stream lst: {stream_lst}")
                    while len(stream_lst) % 2 == 0 and len(stream_lst) > 0:
                        stream_key, stream_entries_lst = stream_lst[:2]
                        stream_lst = stream_lst[2:]

                        if not isinstance(stream_key, str) or not isinstance(
                            stream_entries_lst, list
                        ):
                            logger.warning(
                                f"malformed stream entry: {stream_key}, "
                                + f"{stream_entries_lst}"
                            )
                            continue

                        if stream_key != build_stream:
                            continue

                        for _, stream_entry in stream_entries_lst:
                            log_msg = cast(str | None, stream_entry.get("msg"))
                            if not log_msg:
                                logger.warning(
                                    "unexpected missing 'msg' in stream log entry"
                                )
                                continue

                            _ = await fd.write(f"{log_msg.strip()}\n")

                await fd.flush()

    pass
