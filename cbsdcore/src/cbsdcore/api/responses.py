# CBS service daemon core library - api
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

import uuid
from datetime import datetime as dt

import pydantic

from cbsdcore.versions import BuildDescriptor


class BaseErrorModel(pydantic.BaseModel):
    detail: str


class NewBuildResponse(pydantic.BaseModel):
    build_id: int
    state: str


class AvailableComponent(pydantic.BaseModel):
    name: str
    default_repo: str
    versions: list[str]


AvailableComponentsResponse = dict[str, AvailableComponent]


class PeriodicBuildTaskResponseEntry(pydantic.BaseModel):
    """Represents a periodic build task known to the build service."""

    uuid: uuid.UUID
    enabled: bool
    next_run: dt | None
    created_by: str

    cron_format: str
    tag_format: str
    summary: str | None
    descriptor: BuildDescriptor
