# CBS - builds library - builder
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


import tempfile
from pathlib import Path
from typing import Annotated, override

from cbslib.config.server import Config, get_config
from cbslib.worker import WorkerError
from cbslib.worker.celery import log as parent_logger
from ceslib.runner import runner
from ceslib.versions.desc import VersionDescriptor
from fastapi import Depends

log = parent_logger.getChild("builder")


class BuilderError(WorkerError):
    @override
    def __str__(self) -> str:
        return "Builder Error" + (f": {self.msg}" if self.msg else "")


class BuildInProgressError(WorkerError):
    @override
    def __str__(self) -> str:
        return "Build already in progress"


class Builder:
    _config: Config
    _building: bool

    def __init__(self) -> None:
        self._config = get_config()
        self._building = False

    @property
    def busy(self) -> bool:
        return self._building

    async def build(self, version_desc: VersionDescriptor) -> None:
        if self._building:
            log.debug("build already in progress, ignore")
            raise BuildInProgressError()

        self._building = True

        _, desc_file = tempfile.mkstemp(prefix="cbs_")
        desc_file_path = Path(desc_file)

        with desc_file_path.open("+w") as fd:
            _ = fd.write(version_desc.model_dump_json())

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
                ccache_path=self._config.worker.paths.ccache_path,
                timeout=2 * 60 * 60,  # TODO: make this configurable
            )
            pass
        except Exception as e:
            msg = f"error building '{version_desc.version}': {e}"
            log.error(msg)
            raise BuilderError(msg)
        finally:
            log.info("no longer building")
            self._building = False
            desc_file_path.unlink()

        pass


_builder: Builder | None = None


def builder_init() -> None:
    global _builder
    if not _builder:
        _builder = Builder()


def cbs_builder() -> Builder:
    if not _builder:
        raise BuilderError("missing builder!")
    return _builder


CBSBuilder = Annotated[Builder, Depends(cbs_builder)]
