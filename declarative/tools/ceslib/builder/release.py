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

import asyncio
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
from ceslib.builder.prepare import BuildComponentInfo
from ceslib.builder.upload import S3ComponentLocation, s3_download_json, s3_upload_json
from ceslib.utils import CmdArgs, CommandError, async_run_cmd
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.versions.desc import VersionDescriptor

log = parent_logger.getChild("release")


class ReleaseComponent(pydantic.BaseModel):
    name: str
    version: str
    sha1: str
    repo_url: str

    # s3 locations
    loc: str
    release_rpm_loc: str


class ReleaseDesc(pydantic.BaseModel):
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

    cmd: CmdArgs = [
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


async def release_component_desc(
    components_path: Path,
    component_name: str,
    component_infos: BuildComponentInfo,
    component_s3_locs: S3ComponentLocation,
    build_el_version: int,
) -> ReleaseComponent | None:
    """Create a component release descriptor."""

    rpm_release_loc = await _get_comp_release_rpm(
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
        repo_url=component_infos.repo_url,
        version=component_infos.long_version,
        sha1=component_infos.sha1,
        loc=component_s3_locs.location,
        release_rpm_loc=f"{component_s3_locs.location}/{rpm_release_loc}",
    )


async def release_upload_components(
    secrets: SecretsVaultMgr,
    component_releases: dict[str, ReleaseComponent],
) -> None:
    """Upload component release descriptors to S3, in parallel."""

    log.info(f"upload release for components '{component_releases.keys()}' to S3")

    components_loc = "releases/components"

    async def _put_component(comp_rel: ReleaseComponent) -> str:
        """Write a component's release descriptor to S3."""
        location = f"{components_loc}/{comp_rel.name}/{comp_rel.version}.json"
        data = comp_rel.model_dump_json(indent=2)

        try:
            await s3_upload_json(secrets, location, data)
        except BuilderError as e:
            msg = (
                f"error uploading component release desc for '{comp_rel.name}' "
                + f"to '{location}': {e}"
            )
            log.error(msg)
            raise BuilderError(msg)
        return location

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                name: tg.create_task(_put_component(rel))
                for name, rel in component_releases.items()
            }
    except ExceptionGroup as e:
        log.error("error uploading release descriptors for components:")
        excs = e.subgroup(BuilderError)
        if excs is not None:
            for exc in excs.exceptions:
                log.error(f"- {exc}")
        raise BuilderError("error uploading release descriptors")

    for name, task in task_dict.items():
        log.debug(f"uploaded '{name}' to '{task.result()}'")


async def release_desc_build(
    desc: VersionDescriptor,
    components_info: dict[str, BuildComponentInfo],
    components_path: Path,
    s3_comp_loc: dict[str, S3ComponentLocation],
) -> ReleaseDesc:
    components: dict[str, ReleaseComponent] = {}

    for name, loc in s3_comp_loc.items():
        if name not in components_info:
            msg = f"unexpected missing info for component '{name}'"
            log.error(msg)
            raise BuilderError(msg)

        comp_release = await release_component_desc(
            components_path, name, components_info[name], loc, desc.el_version
        )
        if not comp_release:
            log.error(
                f"unable to find component release RPM location for '{name}', ignore"
            )
            continue

        components[name] = comp_release

    return ReleaseDesc(
        version=desc.version, el_version=desc.el_version, components=components
    )


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


async def check_release_exists(
    secrets: SecretsVaultMgr, version: str
) -> ReleaseDesc | None:
    log.debug(f"check if release '{version}' already exists in S3")

    location = f"releases/{version}.json"
    try:
        data = await s3_download_json(secrets, location)
    except BuilderError as e:
        msg = f"error checking if release '{version}' exists: {e}"
        log.error(msg)
        raise BuilderError(msg)

    if not data:
        return None

    try:
        return ReleaseDesc.model_validate_json(data)
    except pydantic.ValidationError:
        msg = f"invalid release data from '{location}'"
        log.error(msg)
        raise BuilderError(msg)
    except Exception as e:
        msg = f"unknown exception validating release data: {e}"
        log.error(msg)
        raise BuilderError(msg)


async def check_released_components(
    secrets: SecretsVaultMgr, components: dict[str, BuildComponentInfo]
) -> dict[str, ReleaseComponent]:
    """
    Checks whether the components for a release have been previously built and
    uploaded to S3.

    Returns a `dict` mapping names of components existing in S3, to their
    `ReleaseComponent` entry (obtained from S3).

    It's the caller's responsibility to decide whether a component should be built
    or not, regardless of it existing.
    """

    log.debug("check if components exist in S3")

    components_loc = "releases/components"

    async def _get_component(info: BuildComponentInfo) -> ReleaseComponent | None:
        """Obtain `ReleaseComponent` from S3, if available."""
        location = f"{components_loc}/{info.name}/{info.long_version}.json"
        try:
            data = await s3_download_json(secrets, location)
        except BuilderError as e:
            msg = (
                f"error checking if component '{info.name}' "
                + f"version '{info.long_version}' exists in S3: {e}"
            )
            log.error(msg)
            raise BuilderError(msg)

        if not data:
            return None

        try:
            return ReleaseComponent.model_validate_json(data)
        except pydantic.ValidationError:
            msg = f"invalid component release data from '{location}'"
            log.error(msg)
            raise BuilderError(msg)
        except Exception as e:
            msg = f"unknown error validating component release data: {e}"
            log.error(msg)
            raise BuilderError(msg)

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                name: tg.create_task(_get_component(info))
                for name, info in components.items()
            }
    except ExceptionGroup as e:
        log.error("error checking released components:")
        excs = e.subgroup(BuilderError)
        if excs is not None:
            for exc in excs.exceptions:
                log.error(f"- {exc}")
        raise BuilderError("error checking released components")

    existing: dict[str, ReleaseComponent] = {}
    for name, task in task_dict.items():
        res = task.result()
        if not res:
            continue
        log.debug(f"component '{name}' release exists, s3 loc: {res.loc}")
        existing[name] = res

    return existing
