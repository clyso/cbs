# CBS server library -  config - worker
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

from pathlib import Path
from typing import Annotated, ClassVar

import pydantic
from cbscore.config import Config as CBSCoreConfig
from cbscore.config import ConfigError as CBSCoreConfigError
from cbscore.errors import CESError

from cbslib.config import logger as parent_logger

logger = parent_logger.getChild("worker")


class WorkerConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        populate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    config: Path
    cbscore_path: Annotated[Path, pydantic.Field(alias="cbscore-path")]
    build_timeout_seconds: Annotated[
        int | None, pydantic.Field(alias="build-timeout-seconds", default=None)
    ] = None

    def get_cbscore_config(self) -> CBSCoreConfig:
        try:
            return CBSCoreConfig.load(self.config)
        except CBSCoreConfigError as e:
            msg = f"error loading cbscore config from '{self.config}': {e}"
            logger.error(msg)
            raise CESError(msg) from e
