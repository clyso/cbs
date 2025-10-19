# CBS server library -  config - worker
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

from pathlib import Path

import pydantic


class PathsConfig(pydantic.BaseModel):
    cbs_path: Path
    scratch_path: Path
    scratch_container_path: Path
    components_path: list[Path]
    ccache_path: Path


class WorkerConfig(pydantic.BaseModel):
    paths: PathsConfig
    build_timeout_seconds: int | None
