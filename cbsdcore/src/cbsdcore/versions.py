# CBS server library - core - versions
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
from cbscore.versions.utils import VersionType

from cbsdcore.logger import logger as parent_logger

logger = parent_logger.getChild("versions")


class BuildArch(enum.StrEnum):
    """Represents a build architecture."""

    arm64 = "arm64"
    x86_64 = "x86_64"


class BuildArtifactType(enum.StrEnum):
    """Represents a build artifact type."""

    rpm = "rpm"


class BuildSignedOffBy(pydantic.BaseModel):
    """Represents a 'Signed-off-by' from a given user."""

    user: str
    email: str


class BuildDestImage(pydantic.BaseModel):
    """Specifies a given version's destination image's name and tag."""

    name: str
    tag: str


class BuildComponent(pydantic.BaseModel):
    """Represents a component to be built for a given version."""

    name: str
    ref: str
    repo: str | None = pydantic.Field(default=None)


class BuildTarget(pydantic.BaseModel):
    """Represents the build target for a given version."""

    distro: str
    os_version: str
    artifact_type: BuildArtifactType = pydantic.Field(default=BuildArtifactType.rpm)
    arch: BuildArch = pydantic.Field(default=BuildArch.x86_64)


class BuildDescriptor(pydantic.BaseModel):
    """Describes a version to the build service."""

    version: str
    signed_off_by: BuildSignedOffBy
    version_type: VersionType
    dst_image: BuildDestImage
    components: list[BuildComponent]
    build: BuildTarget
