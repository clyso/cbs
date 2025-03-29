# CBS - workqueue's worker - celery
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

import sys

from cbslib.config.server import config_init
from celery import Celery
from ceslib.errors import CESError

from kombu.serialization import register

from cbslib.worker.serializer import pydantic_dumps


celery_app = Celery(
    __name__,
    # include the tasks module, so the worker knows where to find them.
    include=["cbslib.worker.tasks"],
)

log = celery_app.log.get_default_logger(__name__)


def _init() -> None:
    try:
        config = config_init()
    except (CESError, Exception) as e:
        log.error(f"unable to init config: {e}")
        sys.exit(1)

    celery_app.conf.broker_url = config.worker.broker_url
    celery_app.conf.result_backend = config.worker.result_backend_url

    register(
        "pydantic",
        pydantic_dumps,
        None,
        content_type="application/json",
        content_encoding="utf-8",
    )


_init()
