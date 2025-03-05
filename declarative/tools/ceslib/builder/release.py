# CES library - CES builder, track built releases
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
from ceslib.builder import (
    BuilderError,
    get_component_scripts_path,
    get_script_path,
)
from ceslib.builder import (
    log as parent_logger,
)
from ceslib.builder.upload import S3ComponentLocation, s3_upload_json
from ceslib.utils import CommandError, async_run_cmd
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.versions.desc import VersionDescriptor

log = parent_logger.getChild("release")


class ReleaseComponent(pydantic.BaseModel):
    name: str
    version: str
    loc: str
    release_rpm_loc: str


class ReleaseDesc(pydantic.BaseModel):
    version: str
    components: dict[str, ReleaseComponent]

    @classmethod
    def load(cls, path: Path) -> ReleaseDesc:
        try:
            with path.open("r") as f:
                raw_json = f.read()

            return ReleaseDesc.model_validate_json(raw_json)

        except pydantic.ValidationError as e:
            msg = f"error loading container descriptor at '{path}': {e}"
            log.error(msg)
            raise BuilderError(msg)
        except Exception as e:
            msg = f"unknown error loading descriptor at '{path}': {e}"
            log.error(msg)
            raise BuilderError(msg)


async def _get_comp_release_rpm(
    components_path: Path,
    component_name: str,
    el_version: int,
) -> str | None:
    scripts_path = get_component_scripts_path(components_path, component_name)
    if not scripts_path:
        log.warning(
            f"unable to find component release RPM for '{component_name}': "
            + f"no scripts path at '{components_path}"
        )
        return None

    release_rpm_script = get_script_path(scripts_path, "get_release_rpm.*")
    if not release_rpm_script:
        log.warning(
            f"unable to find component release RPM for '{component_name}': "
            + "no script available"
        )
        return None

    cmd = [
        release_rpm_script.resolve().as_posix(),
        str(el_version),
    ]

    try:
        rc, stdout, stderr = await async_run_cmd(cmd)
    except CommandError as e:
        msg = f"error running release RPM script for '{component_name}': {e}"
        log.error(msg)
        raise BuilderError(msg)
    except Exception as e:
        msg = f"unknown error running release RPM script for '{component_name}': {e}"
        log.error(msg)
        raise BuilderError(msg)

    if rc != 0:
        msg = f"error running release RPM script for '{component_name}': {stderr}"
        log.error(msg)
        raise BuilderError(msg)

    return stdout.strip()


async def release_desc_build(
    desc: VersionDescriptor,
    components_path: Path,
    s3_comp_loc: dict[str, S3ComponentLocation],
) -> ReleaseDesc:
    components: dict[str, ReleaseComponent] = {}

    for name, loc in s3_comp_loc.items():
        rpm_release_loc = await _get_comp_release_rpm(
            components_path, name, desc.el_version
        )
        if not rpm_release_loc:
            msg = f"unable to find component release RPM location for '{name}', ignore"
            log.error(msg)
            continue

        components[name] = ReleaseComponent(
            name=name,
            version=loc.version,
            loc=loc.location,
            release_rpm_loc=f"{loc.location}/{rpm_release_loc}",
        )

    return ReleaseDesc(version=desc.version, components=components)


async def release_desc_upload(
    secrets: SecretsVaultMgr, release_desc: ReleaseDesc
) -> None:
    log.debug(f"upload release desc for version '{release_desc.version}' to S3")
    desc_json = release_desc.model_dump_json(indent=2)
    location = f"releases/{release_desc.version}.json"
    try:
        await s3_upload_json(secrets, location, desc_json)
    except BuilderError as e:
        msg = (
            f"error uploading release desc for version '{release_desc.version}' "
            + f"to '{location}': {e}"
        )
        log.error(msg)
        raise BuilderError(msg)
    except Exception as e:
        msg = (
            "unknown error uploading release desc for version "
            + f"'{release_desc.version}' to '{location}': {e}"
        )
        log.error(msg)
        raise BuilderError(msg)
