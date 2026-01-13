# CBS service library - worker
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
from collections.abc import Awaitable
from typing import Any, Literal, cast

import pydantic
from cbscore.runner import stop
from cbsdcore.builds.types import BuildID
from celery import signals

from cbslib.config.config import Config, get_config
from cbslib.config.worker import WorkerConfig
from cbslib.core.backend import Backend, BackendError
from cbslib.worker import WorkerError
from cbslib.worker.celery import logger as parent_logger
from cbslib.worker.types import WorkerBuildEntry, WorkerBuildState, WorkerBuildTask

logger = parent_logger.getChild("worker")


class Worker:
    _backend: Backend
    _config: Config
    _worker_config: WorkerConfig
    _instance_name: str

    def __init__(self, instance_name: str) -> None:
        self._instance_name = instance_name
        self._config = get_config()
        if not self._config.worker:
            msg = "unexpected missing worker config"
            logger.error(msg)
            raise WorkerError(msg)
        self._worker_config = self._config.worker

        try:
            self._backend = Backend(self._config)
        except BackendError as e:
            msg = f"unable to init backend: {e}"
            logger.error(msg)
            raise WorkerError(msg) from e

    @property
    def backend(self) -> Backend:
        return self._backend

    def gc(self) -> None:
        """Garbage collect old builds."""
        pass

    async def start_build(self, task_id: str, entry: WorkerBuildEntry) -> None:
        """Start a build for the worker, keeping track of it."""
        logger.info(f"start build for worker '{self._instance_name}', task '{task_id}'")

        build_task_json = WorkerBuildTask(
            worker_instance_name=self._instance_name,
            task_id=task_id,
            state=WorkerBuildState.STARTED,
            build=entry,
        ).model_dump_json()

        try:
            redis = await self._backend.redis()
            async with redis.pipeline(transaction=True) as pipe:
                _ = pipe.sadd(f"cbs:worker:{self._instance_name}:tasks", task_id)
                _ = pipe.set(f"cbs:worker:tasks:{task_id}", build_task_json)
                _ = pipe.set(f"cbs:builds:{entry.build_id}", build_task_json)
                _ = await pipe.execute()
        except Exception as e:
            msg = f"error starting build: {e}"
            logger.error(msg)
            raise WorkerError(msg) from e

    async def finish_build(
        self,
        task_id: str,
        *,
        revoke: bool = False,
        error: bool = False,
    ) -> None:
        """Finish a build on the worker, cleaning up as necessary."""
        logger.info(f"finish build on worker '{self._instance_name}', task '{task_id}'")

        redis = await self._backend.redis()

        if not await self._with_redis(
            redis.sismember(f"cbs:worker:{self._instance_name}:tasks", task_id)
        ):
            logger.warning(
                f"task '{task_id}' not found in worker's '{self._instance_name}' "
                + "task set -- ignoring request"
            )
            return

        try:
            task_json = cast(
                bytes | None, await redis.get(f"cbs:worker:tasks:{task_id}")
            )
        except Exception as e:
            msg = f"error obtaining task '{task_id}' from redis: {e}"
            logger.error(msg)
            raise WorkerError(msg) from e

        if not task_json:
            logger.error(f"missing task '{task_id}' in redis -- ignore")
            return

        try:
            task = WorkerBuildTask.model_validate_json(task_json)
        except pydantic.ValidationError as e:
            msg = f"error decoding task '{task_id}' from redis:\n{e}"
            logger.error(msg)
            raise WorkerError(msg) from e

        if task.worker_instance_name != self._instance_name:
            logger.warning(
                "attempting to finish a task for another worker "
                + f"'{task.worker_instance_name}', us: '{self._instance_name}' "
                + "-- ignore"
            )
            return

        task.state = WorkerBuildState.FINISHED
        if revoke:
            task.state |= WorkerBuildState.REVOKED
        elif error:
            task.state |= WorkerBuildState.ERROR

        if revoke:
            await self._kill_build(task.build.run_name)

        # cleanup stuff from redis, and update the build's task
        try:
            async with redis.pipeline(transaction=True) as pipe:
                _ = pipe.delete(f"cbs:worker:tasks:{task_id}")
                _ = pipe.srem(f"cbs:worker:{self._instance_name}:tasks", task_id)
                _ = pipe.set(
                    f"cbs:builds:{task.build.build_id}", task.model_dump_json()
                )
                _ = await pipe.execute()
        except Exception as e:
            msg = f"error cleaning up state from redis: {e}"
            logger.error(msg)
            raise WorkerError(msg) from e

    async def _kill_build(self, run_name: str) -> None:
        """Kill an on-going build."""
        try:
            await stop(name=run_name)
            logger.info(f"killed container '{run_name}'")
        except Exception as e:
            msg = f"error stopping '{run_name}': {e}"
            logger.error(msg)
            raise WorkerError(msg) from e

    def terminate_build(self, task_id: str) -> None:
        """Force termination of a given task."""
        try:
            loop = asyncio.get_event_loop()
        except Exception as e:
            msg = f"failed to obtain event loop: {e}"
            logger.error(msg)
            raise WorkerError(msg) from e

        task = loop.create_task(self.finish_build(task_id, revoke=True))
        try:
            loop.run_until_complete(task)
        except Exception as e:
            logger.error(f"failed to terminate task '{task_id}': {e}")
            _ = task.cancel()

    async def log_for_build(self, build_id: BuildID, msg: str) -> None:
        """Store to redis a new log message for a given build."""
        redis = await self._backend.redis()
        await redis.xadd(f"cbs:logs:builds:{build_id}", {"msg": msg})

    async def _with_redis[R, T](self, op: Awaitable[R] | Literal[0, 1]) -> R:
        """
        Handle typing properly for some redis operations.

        This is all about typing issues, no logic changes are applied.
        """
        assert isinstance(op, Awaitable)
        return await op


_worker_instance: Worker | None = None


@signals.celeryd_init.connect
def handle_worker_celeryd_init(sender: str, **kwargs: Any) -> None:  # pyright: ignore[reportExplicitAny, reportAny]
    logger.info(f"initializing worker instance for celeryd: {sender}")
    logger.debug(f"celeryd init -- worker: {sender}, kwargs: {kwargs}")

    global _worker_instance
    assert not _worker_instance, "worker instance already initialized"
    _worker_instance = Worker(sender)
    _worker_instance.gc()


def get_worker() -> Worker:
    """Obtain the worker's instance -- only to be called in worker threads."""
    assert _worker_instance is not None, "worker not initialized"
    return _worker_instance


#
# debug worker init signals
#
@signals.worker_init.connect
def _handle_worker_init(**kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny, reportUnusedFunction]
    logger.debug(f"worker init, kwargs: {kwargs}")


@signals.worker_ready.connect
def _handle_worker_ready(**kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny, reportUnusedFunction]
    logger.debug(f"worker ready, kwargs: {kwargs}")


@signals.worker_before_create_process.connect
def _handle_worker_before_create_process(**kwargs: Any) -> None:  # pyright: ignore[reportExplicitAny, reportAny, reportUnusedFunction]
    logger.debug(f"before create process: {kwargs}")
