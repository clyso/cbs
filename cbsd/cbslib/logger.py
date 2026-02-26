# CBS server library - logging
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
# GNU Affero General Public License for more details.#

# pyright: reportExplicitAny=false

import enum
import logging.config
import os
from copy import deepcopy
from pathlib import Path
from typing import Annotated, Any, ClassVar

import pydantic
import uvicorn.config
from cbscore.errors import CESError
from cbscore.logger import logger as root_logger

DATE_FORMAT = "%Y-%m-%d %H:%M:%S"


class LoggerTarget(enum.StrEnum):
    FILE = "file"
    CONSOLE = "console"


class LoggerConfig(pydantic.BaseModel):
    model_config: ClassVar[pydantic.ConfigDict] = pydantic.ConfigDict(
        populate_by_name=True,
        validate_by_alias=True,
        serialize_by_alias=True,
    )

    target: list[LoggerTarget] = pydantic.Field(default=[LoggerTarget.CONSOLE])
    log_file_path: Annotated[
        Path | None, pydantic.Field(alias="log-file-path", default=None)
    ] = None
    level: str | None = "info"


def _level_name_to_int(name: str) -> int:
    m = logging.getLevelNamesMapping()
    return m.get(name, logging.INFO)


def get_level_from_env(*, default: int | str | None = None) -> int:
    default_value = (
        default
        if default and isinstance(default, int)
        else (_level_name_to_int(default) if default else None)
    ) or logging.INFO
    return default_value if not os.getenv("CBS_DEBUG") else logging.DEBUG


def _get_level_name_from_env(*, default: str | None = None) -> str:
    level = get_level_from_env(default=default)
    return logging.getLevelName(level)


# source: https://gist.github.com/angstwad/bf22d1822c38a92ec0a9?permalink_comment_id=4038517#gistcomment-4038517
# credit to @tfedldmann
# licensed under MIT license
#
def _deep_merge(a: dict[Any, Any], b: dict[Any, Any]) -> dict[Any, Any]:
    result = deepcopy(a)
    for bk, bv in b.items():  # pyright: ignore[reportAny]
        av = result.get(bk)
        if isinstance(av, dict) and isinstance(bv, dict):
            result[bk] = _deep_merge(av, bv)  # pyright: ignore[reportUnknownArgumentType]
        else:
            result[bk] = deepcopy(bv)  # pyright: ignore[reportAny]
    return result


def _get_logging_config(config: LoggerConfig) -> dict[str, Any]:
    level = _get_level_name_from_env(default=config.level).upper()

    if len(config.target) == 0:
        raise CESError("no logging targets configured")

    handlers: list[str] = []
    handlers_cfg: dict[str, Any] = {}

    cfg: dict[str, Any] = {
        "version": 1,
        "disable_existing_loggers": False,
        "formatters": {
            "colorized": {
                "()": "uvicorn.logging.ColourizedFormatter",
                "format": (
                    "%(levelprefix)s %(asctime)s [%(module)s(%(name)s)] %(message)s"
                ),
                "datefmt": DATE_FORMAT,
                "use_colors": True,
            },
            "simple": {
                "format": (
                    "%(levelname)s %(asctime)s [%(module)s(%(name)s)] %(message)s"
                ),
                "datefmt": DATE_FORMAT,
            },
        },
    }

    if LoggerTarget.FILE in config.target:
        if not config.log_file_path:
            raise CESError("logging set to file but no logs dir path specified")

        try:
            config.log_file_path.parent.mkdir(exist_ok=True, parents=True)
        except Exception as e:
            raise CESError(
                f"error creating logs path at '{config.log_file_path}': {e}"
            ) from e

        handlers_cfg["log_file"] = {
            "level": level,
            "class": "logging.handlers.RotatingFileHandler",
            "formatter": "simple",
            "filename": config.log_file_path.as_posix(),
            "maxBytes": 10485760,
            "backupCount": 1,
        }
        handlers.append("log_file")

    if LoggerTarget.CONSOLE in config.target:
        handlers_cfg["console"] = {
            "level": level,
            "class": "logging.StreamHandler",
            "formatter": "colorized",
        }
        handlers.append("console")

    cfg.update(
        {
            "handlers": handlers_cfg,
            "loggers": {
                "uvicorn": {
                    "handlers": handlers,
                    "level": level,
                    "propagate": False,
                },
                "uvicorn.access": {
                    "handlers": handlers,
                    "level": level,
                    "propagate": False,
                },
            },
            "root": {
                "level": level,
                "handlers": handlers,
            },
        }
    )
    return cfg


# uvicorn logging
#
def uvicorn_logging_config() -> dict[str, Any]:
    return _deep_merge(
        uvicorn.config.LOGGING_CONFIG,
        _get_logging_config(LoggerConfig()),
    )


def setup_global_logging(config: LoggerConfig) -> None:
    logging.config.dictConfig(_get_logging_config(config))


def setup_basic_logging() -> None:
    level = _get_level_name_from_env()
    logging.basicConfig(level=level)


# application logger
#
logger = root_logger.getChild("cbs")
