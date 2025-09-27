# CBS - logging
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

import logging.config
import os
from copy import deepcopy
from typing import Any

import uvicorn.config
from ceslib.logger import logger as root_logger

DATE_FORMAT = "%Y-%m-%d %H:%M:%S"


def _get_level_name() -> str:
    level = logging.INFO if not os.getenv("CBS_DEBUG") else logging.DEBUG
    level_name = logging.getLevelName(level)
    return level_name


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


def _setup_logging(
    level: str,
    *,
    log_file: str | None = None,
) -> None:
    file_handler: dict[str, Any] | None = None

    if log_file is not None:
        file_handler = {
            "level": "DEBUG",
            "class": "logging.handlers.RotatingFileHandler",
            "formatter": "simple",
            "filename": log_file,
            "maxBytes": 10485760,
            "backupCount": 1,
        }

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
            },
        },
        "handlers": {
            "console": {
                "level": level,
                "class": "logging.StreamHandler",
                "formatter": "colorized",
            },
        },
    }

    handlers: list[str] = ["console"]

    if file_handler is not None:
        cfg["handlers"]["log_file"] = file_handler
        handlers.append("log_file")

    cfg["root"] = {
        "level": level,
        "handlers": handlers,
    }

    logging.config.dictConfig(cfg)


# uvicorn logging
#
def uvicorn_logging_config() -> dict[str, Any]:
    level = _get_level_name()
    config_dict = {
        "formatters": {
            "default": {
                "fmt": "%(levelprefix)s %(asctime)s -- %(message)s",
                "datefmt": DATE_FORMAT,
            },
            "access": {
                "fmt": '%(levelprefix)s %(asctime)s -- %(client_addr)s -- "%(request_line)s" %(status_code)s',  # noqa: E501
                "datefmt": DATE_FORMAT,
            },
        },
        "handlers": {
            "default": {
                "level": level,
            },
            "access": {
                "level": level,
            },
        },
    }
    #    final_dict = logging_dict | config_dict
    return _deep_merge(uvicorn.config.LOGGING_CONFIG, config_dict)


def setup_logging() -> None:
    level_name = _get_level_name()
    _setup_logging(level_name)


# application logger
#
logger = root_logger.getChild("cbs")
