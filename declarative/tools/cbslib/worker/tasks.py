# CBS - workqueue's worker - tasks
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

from cbslib.worker.builder import Builder, BuilderError
from cbslib.worker.celery import celery_app, log
from ceslib.versions.desc import VersionDescriptor


@celery_app.task(pydantic=True)
def build(version_desc: VersionDescriptor) -> None:
    log.info(f"build version: {version_desc}")

    loop = asyncio.new_event_loop()
    try:
        builder = Builder()
        loop.run_until_complete(builder.build(version_desc))
    except (BuilderError, Exception) as e:
        log.error(f"error running build: {e}")
        return
