# CBS service library - worker - types
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

import pydantic
from cbscore.versions.desc import VersionDescriptor
from cbsdcore.builds.types import BuildID


class WorkerBuildState(enum.IntFlag):
    STARTED = enum.auto()
    FINISHED = enum.auto()
    ERROR = enum.auto()
    REVOKED = enum.auto()


class WorkerBuildEntry(pydantic.BaseModel):
    """Describes a build triggered in a worker process."""

    build_id: BuildID
    run_name: str
    version_desc: VersionDescriptor


class WorkerBuildTask(pydantic.BaseModel):
    """Describes a build task in a given worker instance."""

    worker_instance_name: str
    task_id: str
    state: WorkerBuildState
    build: WorkerBuildEntry
