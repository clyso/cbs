# CES library - build artifact report
# Copyright (C) 2026  Clyso
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

"""
Pydantic models for build artifact reports.

A `BuildArtifactReport` summarises the artifacts produced by a build:
which container image was pushed, where the release descriptor landed
in S3, and which components (with RPM locations) were included.

The report is written to a JSON file inside the build container by
`Builder.run()` and read from the host side by `runner()`, then
propagated through the wrapper, Rust worker, and server.

`report_version` is included for future schema evolution. Consumers
check this field to handle old/new report formats.
"""

import pydantic

from cbscore.builder import logger as parent_logger

logger = parent_logger.getChild("report")


class ContainerImageReport(pydantic.BaseModel):
    """Container image produced by the build."""

    name: str
    """Registry path, e.g. ``harbor.clyso.com/ces-devel/ceph``."""

    tag: str
    """Image tag, e.g. ``v19.2.3-dev.1``."""

    pushed: bool
    """Whether the image was pushed to the registry."""


class ReleaseDescriptorReport(pydantic.BaseModel):
    """Location of the release descriptor in S3."""

    s3_path: str
    """Object key, e.g. ``releases/19.2.3.json``."""

    bucket: str
    """S3 bucket name, e.g. ``cbs-releases``."""


class ComponentReport(pydantic.BaseModel):
    """A single component included in the build."""

    name: str
    """Component name, e.g. ``ceph``."""

    version: str
    """Long version string, e.g. ``19.2.3-42.g5a0b003``."""

    sha1: str
    """Git commit hash of the built source."""

    repo_url: str
    """Source repository URL."""

    rpms_s3_path: str | None = None
    """S3 path to the RPM artifacts (``None`` if not uploaded)."""


class BuildArtifactReport(pydantic.BaseModel):
    """Summary of artifacts produced by a build."""

    report_version: int = 1

    version: str
    """Release version string, e.g. ``19.2.3``."""

    skipped: bool
    """Whether the build was skipped (image already existed)."""

    container_image: ContainerImageReport | None = None
    """Container image info (populated for both skipped and full builds)."""

    release_descriptor: ReleaseDescriptorReport | None = None
    """Release descriptor location in S3 (``None`` when skipped)."""

    components: list[ComponentReport] = []
    """Components included in the build (empty when skipped)."""
