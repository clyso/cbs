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
import tempfile
from pathlib import Path
from typing import override

import pydantic
from cbscore.runner import gen_run_name, runner, stop
from cbscore.versions.desc import VersionDescriptor

from cbslib.config.server import Config, get_config
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


class _WorkerBuildEntry(pydantic.BaseModel):
    run_name: str
    version_desc: VersionDescriptor


class WorkerBuilder:
    _config: Config
    _build: _WorkerBuildEntry | None
    _name: str

    def __init__(self) -> None:
        self._config = get_config()
        self._build = None
        self._name = gen_run_name("cbs_worker_")
        logger.info(f"init builder, name: {self._name}")

    async def pretend_build(self) -> None:
        await asyncio.sleep(300)

    async def pretend_kill(self) -> None:
        logger.info(f"kill builder {self._name}")

    async def build(self, version_desc: VersionDescriptor) -> None:
        if self._build:
            raise WorkerBuilderError(msg="build already exists?")

        _, desc_file = tempfile.mkstemp(prefix="cbs_worker_")
        desc_file_path = Path(desc_file)

        with desc_file_path.open("+w") as fd:
            _ = fd.write(version_desc.model_dump_json())

        self._build = _WorkerBuildEntry(run_name=self._name, version_desc=version_desc)

        try:
            await runner(
                desc_file_path,
                self._config.worker.paths.tools_path,
                self._config.worker.paths.secrets_file_path,
                self._config.worker.paths.scratch_path,
                self._config.worker.paths.scratch_container_path,
                self._config.worker.paths.components_path,
                self._config.worker.paths.containers_path,
                self._config.secrets.vault.addr,
                self._config.secrets.vault.role_id,
                self._config.secrets.vault.secret_id,
                self._config.secrets.vault.transit,
                run_name=self._name,
                ccache_path=self._config.worker.paths.ccache_path,
                timeout=2 * 60 * 60,  # TODO: make this configurable
            )
            pass
        except Exception as e:
            msg = f"error building '{version_desc.version}': {e}"
            logger.exception(msg)
            raise WorkerBuilderError(msg) from e
        finally:
            logger.info("no longer building")
            desc_file_path.unlink()

    async def kill(self) -> None:
        try:
            await stop(name=self._name)
            logger.info(f"killed container '{self._name}'")
        except Exception as e:
            msg = f"error stopping '{self._name}': {e}"
            logger.exception(msg)
            raise WorkerBuilderError(msg) from e
