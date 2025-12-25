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

from cbscore.releases import ReleaseError
from cbscore.releases import logger as parent_logger
from cbscore.releases.desc import ReleaseBuildEntry, ReleaseComponent, ReleaseDesc
from cbscore.utils.s3 import (
    S3Error,
    s3_download_json,
    s3_download_str_obj,
    s3_list,
    s3_upload_json,
)
from cbscore.utils.secrets.mgr import SecretsMgr

logger = parent_logger.getChild("s3")


async def check_release_exists(
    secrets: SecretsMgr, url: str, bucket: str, bucket_loc: str, version: str
) -> ReleaseDesc | None:
    """Check whether a given release version exists in S3."""
    logger.debug(
        f"check if release '{version}' already exists at '{url}' "
        + f"bucket '{bucket}' loc '{bucket_loc}'"
    )

    location = f"{bucket_loc}/{version}.json"
    try:
        data = await s3_download_json(secrets, url, bucket, location)
    except ReleaseError as e:
        msg = f"error checking if release '{version}' exists: {e}"
        logger.exception(msg)
        raise ReleaseError(msg) from e

    if not data:
        return None

    try:
        return ReleaseDesc.model_validate_json(data)
    except pydantic.ValidationError:
        msg = f"invalid release data from '{location}'"
        logger.exception(msg)
        raise ReleaseError(msg) from None
    except Exception as e:
        msg = f"unknown exception validating release data: {e}"
        logger.exception(msg)
        raise ReleaseError(msg) from e


async def release_desc_upload(
    secrets: SecretsMgr,
    url: str,
    bucket: str,
    bucket_loc: str,
    version: str,
    release_build: ReleaseBuildEntry,
) -> ReleaseDesc:
    """Upload a release descriptor to S3."""
    logger.debug(
        f"upload release desc for version '{version}' to '{url}' "
        + f"bucket '{bucket}' loc '{bucket_loc}'"
    )

    try:
        existing_desc = await check_release_exists(
            secrets, url, bucket, bucket_loc, version
        )
    except ReleaseError as e:
        logger.error(f"error checking for existing release '{version}': {e}")
        raise e from None
    except Exception as e:
        msg = f"unknown error checking for existing release '{version}': {e}"
        logger.error(msg)
        raise ReleaseError(msg) from e

    desc = existing_desc or ReleaseDesc(version=version, builds={})
    desc.builds[release_build.arch] = release_build

    desc_json = desc.model_dump_json(indent=2)
    location = f"{bucket_loc}/{version}.json"
    try:
        await s3_upload_json(secrets, url, bucket, location, desc_json)
    except ReleaseError as e:
        msg = (
            f"error uploading release desc for version '{version}' "
            + f"to bucket '{bucket}' loc '{location}': {e}"
        )
        logger.error(msg)
        raise ReleaseError(msg) from e
    except Exception as e:
        msg = (
            f"unknown error uploading release desc for version '{version}'"
            + f"to bucket '{bucket}' loc '{location}': {e}"
        )
        logger.error(msg)
        raise ReleaseError(msg) from e

    return desc


async def release_upload_components(
    secrets: SecretsMgr,
    url: str,
    bucket: str,
    bucket_loc: str,
    component_releases: dict[str, ReleaseComponent],
) -> None:
    """Upload component release descriptors to S3, in parallel."""
    logger.info(
        f"upload release for components '{component_releases.keys()}' to '{url}' "
        + f"bucket '{bucket}' loc '{bucket_loc}'"
    )

    async def _put_component(comp_rel: ReleaseComponent) -> str:
        """Write a component's release descriptor to the provided S3 url."""
        location = f"{bucket_loc}/{comp_rel.name}/{comp_rel.version}.json"
        data = comp_rel.model_dump_json(indent=2)

        try:
            await s3_upload_json(secrets, url, bucket, location, data)
        except ReleaseError as e:
            msg = (
                f"error uploading component release desc for '{comp_rel.name}' "
                + f"bucket '{bucket}' to '{location}': {e}"
            )
            logger.error(msg)
            raise ReleaseError(msg) from e
        return location

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                name: tg.create_task(_put_component(rel))
                for name, rel in component_releases.items()
            }
    except ExceptionGroup as e:
        logger.error("error uploading release descriptors for components:")
        excs = e.subgroup(ReleaseError)
        if excs is not None:
            for exc in excs.exceptions:
                logger.error(f"- {exc}")
        raise ReleaseError(msg="error uploading release descriptors") from e

    for name, task in task_dict.items():
        logger.debug(f"uploaded '{name}' to '{task.result()}'")


async def check_released_components(
    secrets: SecretsMgr,
    url: str,
    bucket: str,
    bucket_loc: str,
    components: dict[str, str],
) -> dict[str, ReleaseComponent]:
    """
    Check whether the components for a release exist in S3.

    Receives a `components` dictionary, mapping the component's name to its version.

    Returns a `dict` mapping names of components existing in S3, to their
    `ReleaseComponent` entry (obtained from S3).

    It's the caller's responsibility to decide whether a component should be built
    or not, regardless of it existing.
    """
    logger.debug(
        f"check if components exist in '{url}' bucket '{bucket}' loc '{bucket_loc}'"
    )

    async def _get_component(name: str, long_version: str) -> ReleaseComponent | None:
        """Obtain `ReleaseComponent` from S3, if available."""
        location = f"{bucket_loc}/{name}/{long_version}.json"
        try:
            data = await s3_download_json(secrets, url, bucket, location)
        except ReleaseError as e:
            msg = (
                f"error checking if component '{name}' "
                + f"version '{long_version}' exists in S3: {e}"
            )
            logger.error(msg)
            raise ReleaseError(msg) from e

        if not data:
            return None

        try:
            return ReleaseComponent.model_validate_json(data)
        except pydantic.ValidationError:
            msg = f"invalid component release data from '{location}'"
            logger.error(msg)
            raise ReleaseError(msg) from None
        except Exception as e:
            msg = f"unknown error validating component release data: {e}"
            logger.error(msg)
            raise ReleaseError(msg) from e

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                name: tg.create_task(_get_component(name, long_version))
                for name, long_version in components.items()
            }
    except ExceptionGroup as e:
        logger.error("error checking released components:")
        excs = e.subgroup(ReleaseError)
        if excs is not None:
            for exc in excs.exceptions:
                logger.error(f"- {exc}")
        raise ReleaseError(msg="error checking released components") from e

    existing: dict[str, ReleaseComponent] = {}
    for name, task in task_dict.items():
        res = task.result()
        if not res:
            continue
        logger.debug(f"component '{name}' release exists, version: {res.version}")
        existing[name] = res

    return existing


async def list_releases(
    secrets: SecretsMgr, url: str, bucket: str, bucket_loc: str
) -> dict[str, ReleaseDesc]:
    """List releases from S3."""
    try:
        res = await s3_list(
            secrets, url, bucket, prefix=f"{bucket_loc}/", prefix_as_directory=True
        )
    except S3Error as e:
        msg = "error obtaining release objects"
        logger.error(msg)
        raise ReleaseError(msg=f"{msg}: {e}") from e

    releases: dict[str, ReleaseDesc] = {}
    for entry in res.objects:
        if not entry.name.endswith(".json"):
            logger.debug(f"skipping '{entry.key}', not JSON")
            continue

        try:
            # FIXME: this is highly inefficient because the 's3_download_str_obj'
            # function will obtain S3 credentials from vault each time.
            raw_json = await s3_download_str_obj(
                secrets, url, bucket, entry.key, content_type=None
            )
        except S3Error as e:
            msg = f"s3 error obtaining JSON object: {e}"
            logger.error(msg)
            raise ReleaseError(msg) from e
        except Exception as e:
            msg = f"unknown error obtaining JSON object: {e}"
            logger.error(msg)
            raise ReleaseError(msg) from e

        if not raw_json:
            continue

        try:
            release_desc = ReleaseDesc.model_validate_json(raw_json)
        except pydantic.ValidationError:
            msg = "malformed or old JSON format for release descriptor"
            logger.error(msg)
            continue

        releases[release_desc.version] = release_desc

    return releases
