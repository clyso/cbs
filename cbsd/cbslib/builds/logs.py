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
import datetime
import logging
import os
import stat
from asyncio.tasks import Task
from datetime import datetime as dt
from pathlib import Path
from typing import Any, cast

import aiofiles
import redis.asyncio as aioredis
from cbscore.errors import CESError
from cbsdcore.builds.types import BuildID

from cbslib.builds import logger as parent_logger
from cbslib.core.backend import Backend

logger = parent_logger.getChild("logs")
# by default, make sure we're not too verbose. This can be adjusted
# if needed by calling the logger's name on server init.
logger.setLevel(logging.INFO)


_BUILD_LOGS_DIR = "builds"
_LOG_STREAM_TTL_SECS = 3600 * 6  # 6 hours


class BuildLogsHandlerError(CESError):
    """Generic build logs handler error."""

    pass


class BuildLogsHandler:
    """Handle logs for all builds."""

    _logs_path: Path
    _backend: Backend
    _tasks: dict[BuildID, Task[None]]
    _finished_streams: dict[dt, list[BuildID]]
    _finished_streams_event: asyncio.Event
    _gc_task: asyncio.Task[None]
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
        self._finished_streams = {}
        self._finished_streams_event = asyncio.Event()
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

        if (env := os.getenv("CBS_DEBUG_BUILDS")) and int(env) == 1:
            logger.setLevel(logging.DEBUG)

        try:
            self._gc_task = asyncio.create_task(
                self._gc_task_fn(), name="log-handler-gc"
            )
        except Exception as e:
            msg = f"error starting log handler gc task: {e}"
            logger.error(msg)
            raise BuildLogsHandlerError(msg) from e

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

            finished_at = dt.now(datetime.UTC)
            if finished_at not in self._finished_streams:
                self._finished_streams[finished_at] = []
            self._finished_streams[finished_at].append(build_id)

            # signal the gc task that we now have at least one value to look out for.
            self._finished_streams_event.set()

    async def shutdown(self) -> None:
        """Shutdown the builds log handler instance."""
        if not self._gc_task:
            return

        _ = self._gc_task.cancel()
        try:
            await self._gc_task
        except asyncio.CancelledError:
            pass
        except Exception as e:
            logger.error(f"error canceling build logs gc task: {e}")

    def get_log_path_for(self, build_id: BuildID) -> Path:
        """Obtain the log file path for a given build."""
        return self._logs_path / f"build-{build_id}.log"

    def _get_build_stream_key(self, build_id: BuildID) -> str:
        """Obtain the stream key for a given build."""
        return f"cbs:logs:builds:{build_id}"

    async def _logger(self, build_id: BuildID) -> None:
        """
        Handle log messages for a given build, writing them to disk.

        Log messages will be consumed from a redis stream, as messages are added to the
        stream by the worker.

        :param BuildID build_id: The ID for the build being handled.
        """
        build_log_path = self.get_log_path_for(build_id)
        build_stream = self._get_build_stream_key(build_id)
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

    async def _gc_task_fn(self) -> None:
        """Garbage collect finished streams after a given TTL."""
        while True:
            try:
                _ = await self._lock.acquire()
                logger.debug("build logs gc fn lock acquired")
            except Exception as e:
                logger.warning(f"error acquiring logs streams lock: {e}")
                await asyncio.sleep(0.01)
                continue

            if len(self._finished_streams) == 0:
                # relinquish lock before waiting on event
                logger.debug("build logs gc fn release lock")
                self._lock.release()
                _ = await self._finished_streams_event.wait()
                self._finished_streams_event.clear()
                # then start from the top
                continue

            try:
                redis = await self._backend.redis()
            except Exception as e:
                logger.error(f"error obtaining redis handle: {e}")
                logger.error("retry in 30 seconds")
                self._lock.release()
                await asyncio.sleep(10)
                # retry from the top
                continue

            time_to_ttl = await self._gc_finished_streams(redis)
            self._lock.release()
            if time_to_ttl:
                logger.info(f"build logs gc waiting for {time_to_ttl} seconds")
                await asyncio.sleep(time_to_ttl)

    async def _gc_finished_streams(self, redis: aioredis.Redis) -> float | None:
        """Garbage collect all finished build log streams."""
        gc_start = dt.now(datetime.UTC)
        logger.info("start gc finished build log streams")
        wait_until: float | None = None
        for finished_at, builds_lst in self._finished_streams.items():
            # check what time it is on each iteration, so we always have an
            # up-to-date value in case we have waited for a significant long
            # time doing gc.
            now = dt.now(datetime.UTC)
            ttl_diff = (now - finished_at).total_seconds()
            if ttl_diff <= _LOG_STREAM_TTL_SECS:
                # nothing to do for the rest of the iterator.
                # return how long we need to wait until the next stream reaches its TTL.
                wait_until = _LOG_STREAM_TTL_SECS - ttl_diff
                break

            for build_id in builds_lst:
                logger.info(
                    f"gc build '{build_id}' logs, exceeded TTL {ttl_diff} seconds"
                )
                await self._gc_stream_key(redis, self._get_build_stream_key(build_id))
            del self._finished_streams[finished_at]

        delta = (dt.now(datetime.UTC) - gc_start).total_seconds()
        logger.info(f"finish gc build log streams in {delta} seconds")
        return wait_until

    async def gc(self, *, all: bool = False) -> None:
        """Garbage collect in-redis log messages from old streams."""
        logger.info("start gc build logs")
        gc_start = dt.now(datetime.UTC)
        async with self._lock:
            logger.debug("build logs gc lock acquired")
            try:
                redis = await self._backend.redis()
            except Exception as e:
                logger.error(f"error obtaining redis handle: {e}")
                logger.error("skip build logs gc")
                return

            if all:
                await self._gc_collect_redis(redis)

            _ = await self._gc_finished_streams(redis)

        total_secs = (dt.now(datetime.UTC) - gc_start).total_seconds()
        logger.info(f"build logs gc took {total_secs} seconds")

    async def _gc_collect_redis(self, redis: aioredis.Redis) -> None:
        """Collect log messages from all existing stream keys."""
        gc_start = dt.now(datetime.UTC)
        total_keys = 0
        cursor_id = 0
        maybe_has_keys = True
        logger.info("start gc redis log streams")
        while maybe_has_keys:
            res: tuple[int, list[str]] = cast(
                tuple[int, list[str]],
                await redis.scan(  # pyright: ignore[reportUnknownMemberType]
                    cursor_id, match="cbs:logs:builds:*", _type="stream"
                ),
            )
            logger.debug(f"gc redis collect: {res}")
            if len(res) != 2:
                logger.warning(f"malformed scan result: {res}")  # pyright: ignore[reportUnreachable]
                break

            cursor_id, key_lst = res
            if cursor_id == 0:
                # A zero cursor id means the redis server signalled that this
                # will be the last iteration.
                logger.debug("no more stream keys from redis to gc")
                maybe_has_keys = False

            for key in key_lst:
                if not key:
                    logger.warning(f"malformed stream key in '{res}'")
                    continue

                await self._gc_stream_key(redis, key)
                total_keys += 1

        total_secs = (dt.now(datetime.UTC) - gc_start).total_seconds()
        logger.info(
            f"gc redis log streams took {total_secs} seconds, {total_keys} keys"
        )

    async def _gc_stream_key(self, redis: aioredis.Redis, stream_key: str) -> None:
        """Collect log messages from a given stream."""
        if await redis.xlen(stream_key) > 0:
            await redis.xtrim(stream_key, maxlen=0)
        pass

    pass
