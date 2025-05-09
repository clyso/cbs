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

import asyncio

import pydantic
from ceslib.releases import ReleaseError
from ceslib.releases import log as parent_logger
from ceslib.releases.desc import ReleaseComponent, ReleaseDesc
from ceslib.utils.s3 import s3_download_json, s3_upload_json
from ceslib.utils.secrets import SecretsVaultMgr

log = parent_logger.getChild("s3")

RELEASES_S3_PATH = "releases"
_RELEASES_COMPONENT_S3_PATH = f"{RELEASES_S3_PATH}/components"


async def release_desc_upload(
    secrets: SecretsVaultMgr, release_desc: ReleaseDesc
) -> None:
    """Upload a release descriptor to S3."""
    log.debug(f"upload release desc for version '{release_desc.version}' to S3")
    desc_json = release_desc.model_dump_json(indent=2)
    location = f"{RELEASES_S3_PATH}/{release_desc.version}.json"
    try:
        await s3_upload_json(secrets, location, desc_json)
    except ReleaseError as e:
        msg = (
            f"error uploading release desc for version '{release_desc.version}' "
            + f"to '{location}': {e}"
        )
        log.exception(msg)
        raise ReleaseError(msg) from e
    except Exception as e:
        msg = (
            "unknown error uploading release desc for version "
            + f"'{release_desc.version}' to '{location}': {e}"
        )
        log.exception(msg)
        raise ReleaseError(msg) from e


async def release_upload_components(
    secrets: SecretsVaultMgr,
    component_releases: dict[str, ReleaseComponent],
) -> None:
    """Upload component release descriptors to S3, in parallel."""
    log.info(f"upload release for components '{component_releases.keys()}' to S3")

    async def _put_component(comp_rel: ReleaseComponent) -> str:
        """Write a component's release descriptor to S3."""
        location = (
            f"{_RELEASES_COMPONENT_S3_PATH}/{comp_rel.name}/{comp_rel.version}.json"
        )
        data = comp_rel.model_dump_json(indent=2)

        try:
            await s3_upload_json(secrets, location, data)
        except ReleaseError as e:
            msg = (
                f"error uploading component release desc for '{comp_rel.name}' "
                + f"to '{location}': {e}"
            )
            log.exception(msg)
            raise ReleaseError(msg) from e
        return location

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                name: tg.create_task(_put_component(rel))
                for name, rel in component_releases.items()
            }
    except ExceptionGroup as e:
        log.error("error uploading release descriptors for components:")  # noqa: TRY400
        excs = e.subgroup(ReleaseError)
        if excs is not None:
            for exc in excs.exceptions:
                log.error(f"- {exc}")  # noqa: TRY400
        raise ReleaseError(msg="error uploading release descriptors") from e

    for name, task in task_dict.items():
        log.debug(f"uploaded '{name}' to '{task.result()}'")


async def check_release_exists(
    secrets: SecretsVaultMgr, version: str
) -> ReleaseDesc | None:
    """Check whether a given release version exists in S3."""
    log.debug(f"check if release '{version}' already exists in S3")

    location = f"{RELEASES_S3_PATH}/{version}.json"
    try:
        data = await s3_download_json(secrets, location)
    except ReleaseError as e:
        msg = f"error checking if release '{version}' exists: {e}"
        log.exception(msg)
        raise ReleaseError(msg) from e

    if not data:
        return None

    try:
        return ReleaseDesc.model_validate_json(data)
    except pydantic.ValidationError:
        msg = f"invalid release data from '{location}'"
        log.exception(msg)
        raise ReleaseError(msg) from None
    except Exception as e:
        msg = f"unknown exception validating release data: {e}"
        log.exception(msg)
        raise ReleaseError(msg) from e


async def check_released_components(
    secrets: SecretsVaultMgr, components: dict[str, str]
) -> dict[str, ReleaseComponent]:
    """
    Check whether the components for a release exist in S3.

    Receives a `components` dictionary, mapping the component's name to its version.

    Returns a `dict` mapping names of components existing in S3, to their
    `ReleaseComponent` entry (obtained from S3).

    It's the caller's responsibility to decide whether a component should be built
    or not, regardless of it existing.
    """
    log.debug("check if components exist in S3")

    async def _get_component(name: str, long_version: str) -> ReleaseComponent | None:
        """Obtain `ReleaseComponent` from S3, if available."""
        location = f"{_RELEASES_COMPONENT_S3_PATH}/{name}/{long_version}.json"
        try:
            data = await s3_download_json(secrets, location)
        except ReleaseError as e:
            msg = (
                f"error checking if component '{name}' "
                + f"version '{long_version}' exists in S3: {e}"
            )
            log.exception(msg)
            raise ReleaseError(msg) from e

        if not data:
            return None

        try:
            return ReleaseComponent.model_validate_json(data)
        except pydantic.ValidationError:
            msg = f"invalid component release data from '{location}'"
            log.exception(msg)
            raise ReleaseError(msg) from None
        except Exception as e:
            msg = f"unknown error validating component release data: {e}"
            log.exception(msg)
            raise ReleaseError(msg) from e

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                name: tg.create_task(_get_component(name, long_version))
                for name, long_version in components.items()
            }
    except ExceptionGroup as e:
        log.error("error checking released components:")  # noqa: TRY400
        excs = e.subgroup(ReleaseError)
        if excs is not None:
            for exc in excs.exceptions:
                log.error(f"- {exc}")  # noqa: TRY400
        raise ReleaseError(msg="error checking released components") from e

    existing: dict[str, ReleaseComponent] = {}
    for name, task in task_dict.items():
        res = task.result()
        if not res:
            continue
        log.debug(f"component '{name}' release exists, s3 loc: {res.loc}")
        existing[name] = res

    return existing
