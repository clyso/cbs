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

import pydantic

from cbsdcore.versions import BuildDescriptor


class NewPeriodicBuildTaskRequest(pydantic.BaseModel):
    """Describes a new periodic build task to the API."""

    cron_format: str
    tag_format: str
    descriptor: BuildDescriptor
    summary: str | None = pydantic.Field(default=None)
