# CES library - CES releases
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.

from __future__ import annotations

import enum
from pathlib import Path

import pydantic

from cbscore.releases import ReleaseError
from cbscore.releases import logger as parent_logger

logger = parent_logger.getChild("desc")


class ArchType(enum.StrEnum):
    x86_64 = "x86_64"


class BuildType(enum.StrEnum):
    rpm = "rpm"


class BuildInfo(pydantic.BaseModel):
    """Specify a given build's specific information."""

    arch: ArchType
    build_type: BuildType
    os_version: str


class ReleaseComponentHeader(pydantic.BaseModel):
    """Specify information about a given release component."""

    name: str
    version: str
    sha1: str


class ReleaseRPMArtifacts(pydantic.BaseModel):
    """Specify an RPM release's artifacts locations in S3."""

    loc: str
    release_rpm_loc: str


# allow extending this type, possibly including discriminators,
# should we want to add other build types in the future.
#
ReleaseArtifacts = ReleaseRPMArtifacts


class ReleaseComponentVersion(ReleaseComponentHeader, BuildInfo):
    """
    Describe a version of a given released component.

    Extends 'ReleaseComponentHeader' for the component's name, version, and sha1.
    Extends 'BuildInfo' for the build's architecture, build type, and OS version.

    Specifies the repository URL from which the source was obtained, as well as
    the locations for the generated artifacts.
    """

    # source repository
    repo_url: str
    artifacts: ReleaseArtifacts


class ReleaseComponent(ReleaseComponentHeader):
    """
    Describe a release component's various builds, at a specific version.

    A release component refers to a given built component, as part of a release,
    and contains the component's name, its version, the SHA1 corresponding to the
    built source.

    Each released component will contain one or more versions, each describing a
    specific build for the component. These will include the source repository,
    the build type (only rpm at the moment), the OS version it was built for, and
    the build architecture. This ensures a given component version can be built for
    different OS versions and architectures, while keeping track of the required RPMs.
    """

    versions: list[ReleaseComponentVersion]


ReleaseComponentSet = pydantic.TypeAdapter(list[ReleaseComponent])


class ReleaseBuildEntry(BuildInfo):
    """
    Describe a release's build.

    A release build is specific to a given architecture, build type (rpm, etc), and
    an OS version (el9, el10, etc).

    It points to the various components built for during this build, so we known
    where to find the corresponding build artifacts that match the release's
    architecture, build type, and OS version.

    Note: this class extends 'BuildInfo', which specify the architecture, build
    type, and OS version.
    """

    components: dict[str, ReleaseComponentVersion]


class ReleaseDesc(pydantic.BaseModel):
    """
    Describe a given release.

    A release is composed by a given version, and a number of release builds, one per
    architecture type.
    """

    version: str
    builds: dict[ArchType, ReleaseBuildEntry]

    @classmethod
    def load(cls, path: Path) -> ReleaseDesc:
        try:
            with path.open("r") as f:
                raw_json = f.read()

            return ReleaseDesc.model_validate_json(raw_json)

        except pydantic.ValidationError as e:
            msg = f"error loading container descriptor at '{path}': {e}"
            logger.exception(msg)
            raise ReleaseError(msg) from e
        except Exception as e:
            msg = f"unknown error loading descriptor at '{path}': {e}"
            logger.exception(msg)
            raise ReleaseError(msg) from e


# async def release_component_desc(
#     component_loc: CoreComponentLoc,
#     component_name: str,
#     repo_url: str,
#     long_version: str,
#     sha1: str,
#     s3_location: str,
#     build_el_version: int,
# ) -> ReleaseComponent | None:
#     """Create a component release descriptor."""
#     if not component_loc.comp.build.rpm:
#         msg = f"component '{component_name}' has no 'build.rpm' section"
#         logger.error(msg)
#         return None
#
#     rpm_release = component_loc.path / component_loc.comp.build.rpm.release_rpm
#     if not rpm_release.exists() or not rpm_release.is_file():
#         msg = (
#             f"component '{component_name}' has no release RPM script at '{rpm_release}'"
#         )
#         logger.error(msg)
#         return None
#
#     rpm_release_loc = await get_component_release_rpm(component_loc, build_el_version)
#     if not rpm_release_loc:
#         msg = (
#             "unable to find component release RPM location "
#             + f"for '{component_name}', "
#             + f"el version: {build_el_version}"
#         )
#         logger.error(msg)
#         return None
#
#     return ReleaseComponent(
#         name=component_name,
#         version=long_version,
#         sha1=sha1,
#         repo_url=repo_url,
#         artifacts=ReleaseRPMArtifacts(
#             loc=s3_location,
#             release_rpm_loc=f"{s3_location}/{rpm_release_loc}",
#         ),
#     )
