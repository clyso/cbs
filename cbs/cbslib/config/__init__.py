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
from typing import Annotated

import pydantic
from fastapi import Depends

from cbscore.errors import CESError
from cbslib.config.server import ServerConfig
from cbslib.config.worker import WorkerConfig


class Config(pydantic.BaseModel):
    vault_config: Path
    server: ServerConfig | None = pydantic.Field(default=None)
    worker: WorkerConfig | None = pydantic.Field(default=None)
    broker_url: str
    result_backend_url: str
    secrets_file_path: Path

    @classmethod
    def load(cls, *, path: Path | None = None) -> Config:
        env_conf = os.getenv("CBS_CONFIG")
        env_conf_path = Path(env_conf) if env_conf else None
        config_path = path if path else env_conf_path
        if not config_path:
            raise CESError(msg="missing config")

        if not config_path.exists():
            raise CESError(msg=f"config at '{config_path}' does not exist")

        with config_path.open("r") as f:
            try:
                return Config.model_validate_json(f.read())
            except pydantic.ValidationError as e:
                raise CESError(
                    msg=f"malformed config at '{config_path}': {e}"
                ) from None
            except Exception as e:
                raise CESError(
                    msg=f"unexpected error loading config at '{config_path}': {e}"
                ) from e


_config: Config | None = None


def config_init() -> Config:
    global _config
    if _config:
        return _config

    _config = Config.load()
    return _config


def cbs_config() -> Config:
    if not _config:
        raise CESError(msg="config not set!")
    return _config


def get_config() -> Config:
    if not _config:
        raise CESError(msg="config not set!")
    return _config.model_copy(deep=True)


CBSConfig = Annotated[Config, Depends(cbs_config)]
