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

from pathlib import Path

import pydantic
from ceslib.releases import ReleaseError
from ceslib.releases import log as parent_logger
from ceslib.releases.utils import get_component_release_rpm

log = parent_logger.getChild("desc")


class ReleaseComponent(pydantic.BaseModel):
    """
    Describe a release component.

    A release component refers to a given built component, as part of a release,
    and contains the component's name, its version, the SHA1 corresponding to the
    built source, and the repository the component was consumed from. Additionally,
    includes the S3 location for the component's artifacts, including the component's
    release RPM to be installed (which will usually contain the .repo file to install
    the component's artifacts).
    """

    name: str
    version: str
    sha1: str
    repo_url: str

    # s3 locations
    loc: str
    release_rpm_loc: str


class ReleaseDesc(pydantic.BaseModel):
    """
    Describe a given release.

    A release is composed by a given version, an el version, and a set of components.
    These descriptors will usually live in an S3 backend, as JSON objects.
    """

    version: str
    el_version: int
    components: dict[str, ReleaseComponent]

    @classmethod
    def load(cls, path: Path) -> ReleaseDesc:
        try:
            with path.open("r") as f:
                raw_json = f.read()

            return ReleaseDesc.model_validate_json(raw_json)

        except pydantic.ValidationError as e:
            msg = f"error loading container descriptor at '{path}': {e}"
            log.exception(msg)
            raise ReleaseError(msg) from e
        except Exception as e:
            msg = f"unknown error loading descriptor at '{path}': {e}"
            log.exception(msg)
            raise ReleaseError(msg) from e


async def release_component_desc(
    components_path: Path,
    component_name: str,
    repo_url: str,
    long_version: str,
    sha1: str,
    s3_location: str,
    build_el_version: int,
) -> ReleaseComponent | None:
    """Create a component release descriptor."""
    rpm_release_loc = await get_component_release_rpm(
        components_path,
        component_name,
        build_el_version,
    )
    if not rpm_release_loc:
        msg = (
            "unable to find component release RPM location "
            + f"for '{component_name}', "
            + f"el version: {build_el_version}"
        )
        log.error(msg)
        return None

    return ReleaseComponent(
        name=component_name,
        repo_url=repo_url,
        version=long_version,
        sha1=sha1,
        loc=s3_location,
        release_rpm_loc=f"{s3_location}/{rpm_release_loc}",
    )
