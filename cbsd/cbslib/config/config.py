# CBS server library - config
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

from __future__ import annotations

import os
from pathlib import Path
from typing import Annotated, ClassVar

import pydantic
import yaml
from cbscore.errors import CESError
from fastapi import Depends

from cbslib.config import logger as parent_logger
from cbslib.config.server import ServerConfig
from cbslib.config.worker import WorkerConfig

logger = parent_logger.getChild("config")


class Config(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        populate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    server: ServerConfig | None = pydantic.Field(default=None)
    worker: WorkerConfig | None = pydantic.Field(default=None)
    broker_url: Annotated[str, pydantic.Field(alias="broker-url")]
    results_backend_url: Annotated[str, pydantic.Field(alias="results-backend-url")]
    redis_backend_url: Annotated[str, pydantic.Field(alias="redis-backend-url")]

    @classmethod
    def load(cls, *, path: Path | None = None) -> Config:
        env_conf = os.getenv("CBS_CONFIG")
        env_conf_path = Path(env_conf) if env_conf else None
        path = path if path else env_conf_path
        if not path:
            msg = "missing config"
            logger.error(msg)
            raise CESError(msg)

        if not path.exists() or not path.is_file():
            msg = f"config at '{path}' is not a file or does not exist"
            logger.error(msg)
            raise CESError(msg)

        if path.suffix.lower() not in [".json", ".yaml", ".yml"]:
            msg = f"unsupported config file type '{path.suffix}' at '{path}'"
            logger.error(msg)
            raise CESError(msg)

        try:
            raw_data = path.read_text()
            return Config.model_validate(yaml.safe_load(raw_data))
        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading config at '{path}': {e}"
            logger.error(msg)
            raise CESError(msg) from e
        except Exception as e:
            msg = f"unexpected error loading config at '{path}': {e}"
            logger.error(msg)
            raise CESError(msg) from e


_config: Config | None = None


def config_init() -> Config:
    global _config
    if _config:
        return _config

    _config = Config.load()
    return _config


def cbs_config() -> Config:
    if not _config:
        msg = "config not set!"
        logger.error(msg)
        raise CESError(msg)
    return _config


def get_config() -> Config:
    return cbs_config().model_copy(deep=True)


CBSConfig = Annotated[Config, Depends(cbs_config)]
