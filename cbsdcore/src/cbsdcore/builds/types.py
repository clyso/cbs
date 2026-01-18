# CBS service daemon core library - builds
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

import enum
from datetime import datetime as dt

import pydantic

from cbsdcore.versions import BuildDescriptor


class EntryState(str, enum.Enum):
    #
    # NEW represents a newly created build entry, but not yet scheduled.
    # This is an internal state only, and should just exist between the time
    # the build entry is created and the time it is scheduled for execution.
    #
    new = "NEW"
    #
    # the following states map to Celery task states.
    #
    pending = "PENDING"
    started = "STARTED"
    retry = "RETRY"
    failure = "FAILURE"
    success = "SUCCESS"
    revoked = "REVOKED"
    rejected = "REJECTED"


class BuildEntry(pydantic.BaseModel):
    task_id: str | None = None
    desc: BuildDescriptor
    user: str
    submitted: dt
    state: EntryState
    started: dt | None
    finished: dt | None


BuildID = int
