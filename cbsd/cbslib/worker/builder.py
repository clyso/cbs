# CBS server library - builds library - builder
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
import re
import tempfile
from pathlib import Path
from typing import Any, override

from cbscore.config import Config as CBSCoreConfig
from cbscore.errors import MalformedVersionError
from cbscore.runner import gen_run_name, runner
from cbscore.versions.create import version_create_helper
from cbscore.versions.desc import VersionDescriptor
from cbscore.versions.errors import VersionError
from cbsdcore.builds.types import BuildID
from cbsdcore.versions import BuildDescriptor
from celery import signals

from cbslib.config.config import Config, get_config
from cbslib.config.worker import WorkerConfig
from cbslib.worker import WorkerError
from cbslib.worker.celery import logger as parent_logger
from cbslib.worker.types import WorkerBuildEntry
from cbslib.worker.worker import Worker, get_worker

logger = parent_logger.getChild("builder")


class WorkerBuilderError(WorkerError):
    @override
    def __str__(self) -> str:
        return "Builder Error" + (f": {self.msg}" if self.msg else "")


class WorkerBuildInProgressError(WorkerError):
    @override
    def __str__(self) -> str:
        return "Build already in progress"


class BuildOSVersionNotPermittedError(WorkerBuilderError):
    @override
    def __str__(self) -> str:
        return "OS version not permitted" + (f": {self.msg}" if self.msg else "")


def _create_version_desc(
    build_desc: BuildDescriptor, config: CBSCoreConfig
) -> VersionDescriptor:
    """Obtain a VersionDescriptor for the build from the provided BuildDescriptor."""
    # FIXME: this is quite the kludge, but that's because underneath (in cbscore et al.)
    # we don't actually support different OS'es othern than EL-based.
    os_version_m = re.match(r"^el(\d+)$", build_desc.build.os_version)
    if not os_version_m:
        msg = f"unknown OS version '{build_desc.build.os_version}'"
        logger.error(msg)
        raise BuildOSVersionNotPermittedError(msg)

    el_version = int(os_version_m.group(1))

    if not config.storage or not config.storage.registry:
        msg = "registry not specified in config, don't build"
        logger.error(msg)
        raise WorkerBuilderError(msg)

    try:
        return version_create_helper(
            version=build_desc.version,
            version_type_name=build_desc.version_type.value,
            component_refs={c.name: c.ref for c in build_desc.components},
            components_paths=config.paths.components,
            component_uri_overrides={
                c.name: c.repo for c in build_desc.components if c.repo is not None
            },
            distro=build_desc.build.distro,
            el_version=el_version,
            registry=config.storage.registry.url,
            image_name=build_desc.dst_image.name,
            image_tag=build_desc.dst_image.tag,
            user_name=build_desc.signed_off_by.user,
            user_email=build_desc.signed_off_by.email,
        )
    except VersionError as e:
        msg = f"error creating version descriptor for build: {e}"
        logger.error(msg)
        raise WorkerBuilderError(msg) from e
    except MalformedVersionError as e:
        logger.error(f"malformed version while creating version descriptor: {e}")
        raise e from None


class WorkerBuilder:
    """Handles builds in a worker node."""

    _config: Config
    _worker_config: WorkerConfig
    _cbscore_config: CBSCoreConfig
    _worker: Worker
    _name: str
    _build_task: asyncio.Task[None] | None
    # asyncio loop for a specific worker thread.
    # this is not the same event loop as the worker's main process.
    _our_loop: asyncio.AbstractEventLoop

    def __init__(self, worker: Worker) -> None:
        self._worker = worker
        self._config = get_config()
        if not self._config.worker:
            msg = "unexpected missing worker config"
            logger.error(msg)
            raise WorkerBuilderError(msg)

        self._worker_config = self._config.worker
        self._cbscore_config = self._worker_config.get_cbscore_config()
        self._name = gen_run_name("cbs_worker_")

        self._build_task = None
        self._our_loop = asyncio.new_event_loop()

        logger.info(f"init worker builder, name: {self._name}")

        if not self._config.broker_url or not self._config.results_backend_url:
            msg = "broker or result backend url missing from config"
            logger.error(msg)
            raise WorkerBuilderError(msg)

    def build(
        self,
        task_id: str,
        build_id: BuildID,
        build_desc: BuildDescriptor,
    ) -> None:
        """Start a build in the worker node."""
        assert self._our_loop, "Missing event loop for worker builder"
        if self._build_task:
            msg = "on-going build task found, ignore build request"
            logger.error(msg)
            raise WorkerBuilderError(msg)

        asyncio.set_event_loop(self._our_loop)
        self._build_task = self._our_loop.create_task(
            self._do_build(task_id, build_id, build_desc)
        )

        try:
            self._our_loop.run_until_complete(self._build_task)
        except Exception as e:
            logger.error(f"build task failed: {e}")
            _ = self._build_task.cancel()
        finally:
            self._build_task = None

    async def _do_build(
        self,
        task_id: str,
        build_id: BuildID,
        build_desc: BuildDescriptor,
    ) -> None:
        """Run a build in the worker node."""
        logger.debug(
            f"starting build '{build_id}' for version '{build_desc.version}', "
            + f"task '{task_id}'"
        )

        try:
            version_desc = _create_version_desc(build_desc, self._cbscore_config)
        except WorkerBuilderError as e:
            msg = f"error creating version descriptor for build: {e}"
            logger.error(msg)
            raise WorkerBuilderError(msg) from e
        except MalformedVersionError as e:
            logger.error(f"error creating version descriptor for build: {e}")
            raise e from None
        except Exception as e:
            msg = f"unknown error creating version descriptor for build: {e}"
            logger.error(msg)
            raise WorkerBuilderError(msg) from e

        _, desc_file = tempfile.mkstemp(prefix="cbs_worker_")
        desc_file_path = Path(desc_file)

        with desc_file_path.open("+w") as fd:
            _ = fd.write(version_desc.model_dump_json())

        build_entry = WorkerBuildEntry(
            build_id=build_id,
            run_name=self._name,
            version_desc=version_desc,
        )

        await self._worker.start_build(task_id, build_entry)

        has_error = False
        try:
            await runner(
                desc_file_path,
                self._worker_config.cbscore_path,
                self._cbscore_config,
                run_name=self._name,
                replace_run=True,
                timeout=(
                    self._worker_config.build_timeout_seconds
                    if self._worker_config.build_timeout_seconds
                    else 2 * 60 * 60
                ),
            )
        except Exception as e:
            msg = f"error building '{version_desc.version}': {e}"
            logger.error(msg)
            has_error = True
            raise WorkerBuilderError(msg) from e
        finally:
            logger.info("no longer building")
            desc_file_path.unlink()
            await self._worker.finish_build(task_id, error=has_error)


# the worker's builder will only be initialized at individual worker process init.
# this will be handled by the signal handler later in this file.
#
_worker_builder: WorkerBuilder | None = None


@signals.worker_process_init.connect
def handle_worker_process_init(**_kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny]
    #
    # We will have one instance of 'WorkerBuilder' per worker pool process.
    #
    logger.debug("worker process init, initialize builder")
    global _worker_builder
    if not _worker_builder:
        _worker_builder = WorkerBuilder(get_worker())


def get_builder() -> WorkerBuilder:
    """Obtain the worker's builder class -- only to be called in worker threads."""
    assert _worker_builder, "expected worker builder to be defined!!"
    return _worker_builder
