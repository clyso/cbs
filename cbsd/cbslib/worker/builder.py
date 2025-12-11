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

import pydantic
from celery import signals

from cbscore.config import Config as CBSCoreConfig
from cbscore.errors import MalformedVersionError
from cbscore.runner import gen_run_name, runner, stop
from cbscore.versions.create import version_create_helper
from cbscore.versions.desc import VersionDescriptor
from cbscore.versions.errors import VersionError
from cbsdcore.versions import BuildDescriptor
from cbslib.config.config import Config, get_config
from cbslib.worker import WorkerError
from cbslib.worker.celery import logger as parent_logger

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


class _WorkerBuildEntry(pydantic.BaseModel):
    run_name: str
    version_desc: VersionDescriptor


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

    if not config.secrets_config or not config.secrets_config.registry:
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
            registry=config.secrets_config.registry,
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
    _cbscore_config: CBSCoreConfig
    _build: _WorkerBuildEntry | None
    _name: str

    def __init__(self) -> None:
        self._config = get_config()
        if not self._config.worker:
            msg = "unexpected missing worker config"
            logger.error(msg)
            raise WorkerBuilderError(msg)

        self._cbscore_config = self._config.worker.get_cbscore_config()
        self._build = None
        self._name = gen_run_name("cbs_worker_")
        logger.info(f"init builder, name: {self._name}")

        if not self._config.broker_url or not self._config.results_backend_url:
            msg = "broker or result backend url missing from config"
            logger.error(msg)
            raise WorkerBuilderError(msg)

    async def pretend_build(self) -> None:
        await asyncio.sleep(300)

    async def pretend_kill(self) -> None:
        logger.info(f"kill builder {self._name}")

    async def build(self, build_desc: BuildDescriptor) -> None:
        """Start a build in the worker node."""
        if not self._config.worker:
            msg = "worker config missing"
            logger.error(msg)
            raise WorkerBuilderError(msg)

        logger.debug(f"starting build for version '{build_desc.version}'")
        if self._build:
            raise WorkerBuilderError(msg="already building?")

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

        self._build = _WorkerBuildEntry(run_name=self._name, version_desc=version_desc)

        try:
            await runner(
                desc_file_path,
                self._config.worker.cbscore_path,
                self._cbscore_config,
                run_name=self._name,
                replace_run=True,
                timeout=(
                    self._config.worker.build_timeout_seconds
                    if self._config.worker.build_timeout_seconds
                    else 2 * 60 * 60
                ),
            )
        except Exception as e:
            msg = f"error building '{version_desc.version}': {e}"
            logger.error(msg)
            raise WorkerBuilderError(msg) from e
        finally:
            logger.info("no longer building")
            desc_file_path.unlink()
            self._build = None

    async def kill(self) -> None:
        """Kill an on-going build."""
        try:
            await stop(name=self._name)
            logger.info(f"killed container '{self._name}'")
        except Exception as e:
            msg = f"error stopping '{self._name}': {e}"
            logger.error(msg)
            raise WorkerBuilderError(msg) from e
        finally:
            self._build = None


_worker_builder: WorkerBuilder | None = None


@signals.worker_init.connect
def handle_worker_init(**_kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny]
    logger.info("worker init -- init builder")
    global _worker_builder
    if not _worker_builder:
        _worker_builder = WorkerBuilder()


@signals.worker_process_init.connect
def handle_worker_process_init(**_kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny]
    logger.debug("worker process init")


@signals.worker_ready.connect
def handle_worker_ready(**_kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny]
    logger.debug("worker ready")


@signals.worker_shutting_down.connect
def handler_worker_shutting_down(*args: Any, **kwargs: Any) -> None:  # pyright: ignore[reportAny, reportExplicitAny]
    logger.info(f"worker shutting down, args: {args}, kwargs: {kwargs}")


def get_builder() -> WorkerBuilder:
    """Obtain the worker's builder class -- only to be called in worker threads."""
    assert _worker_builder, "expected worker builder to be defined!!"
    return _worker_builder
