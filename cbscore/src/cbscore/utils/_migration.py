# CES library - migration utilities
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

import errno
import re
from typing import cast

import pydantic

from cbscore.errors import CESError
from cbscore.releases.desc import (
    ArchType,
    BuildType,
    ReleaseBuildEntry,
    ReleaseComponent,
    ReleaseComponentVersion,
    ReleaseDesc,
    ReleaseRPMArtifacts,
)
from cbscore.utils import logger as parent_logger
from cbscore.utils.s3 import s3_download_str_obj, s3_list, s3_upload_json
from cbscore.utils.secrets import SecretsVaultMgr

logger = parent_logger.getChild("migration")


class MigrationError(CESError):
    code: int

    def __init__(self, code: int, message: str | None = None) -> None:
        super().__init__(message)
        self.code = code


class ReleaseComponentV1(pydantic.BaseModel):
    name: str
    version: str
    sha1: str = pydantic.Field(default="")
    repo_url: str = pydantic.Field(default="")
    loc: str
    release_rpm_loc: str


class ReleaseDescV1(pydantic.BaseModel):
    version: str
    el_version: int = pydantic.Field(default=9)
    components: dict[str, ReleaseComponentV1]


async def _migrate_components_v1(secrets: SecretsVaultMgr) -> None:
    components_path = "releases/components"

    try:
        components_lst_res = await s3_list(
            secrets, prefix=f"{components_path}/", prefix_as_directory=True
        )
    except Exception as e:
        logger.error(f"error listing components from s3: {e}")
        raise MigrationError(errno.ENOTRECOVERABLE) from e

    logger.info(f"=> migrate {len(components_lst_res.common_prefixes)} components")
    for entry in components_lst_res.common_prefixes:
        component_name = entry[len(components_path) + 1 : -1]
        logger.info(f"> migrate '{component_name}'")

        try:
            component_versions_res = await s3_list(
                secrets, prefix=entry, prefix_as_directory=True
            )
        except Exception as e:
            logger.error(f"error listing component '{component_name}': {e}")
            raise MigrationError(errno.ENOTRECOVERABLE) from e

        for version_entry in component_versions_res.objects:
            if not version_entry.key.endswith(".json"):
                logger.warning(f"skipping non-JSON component file: {version_entry.key}")
                continue

            try:
                data = await s3_download_str_obj(
                    secrets, version_entry.key, content_type=None
                )
            except Exception as e:
                logger.error(f"unable to download '{version_entry.key}': {e}")
                raise MigrationError(errno.ENOTRECOVERABLE) from e

            if not data:
                logger.warning(f"found empty component file: {version_entry.key}")
                continue

            try:
                _ = ReleaseComponent.model_validate_json(data)
                logger.info(f"already migrated component file: {version_entry.key}")
                continue
            except pydantic.ValidationError:
                logger.info(f"need to migrate '{version_entry.key}'")
            except Exception as e:
                logger.error(f"error checking '{version_entry.key}': {e}")
                raise MigrationError(errno.ENOTRECOVERABLE) from e

            logger.info(f"migrating component file '{version_entry.key}'")
            try:
                old_ver = ReleaseComponentV1.model_validate_json(data)
                logger.info(f"  loaded old version component file: {version_entry.key}")
            except pydantic.ValidationError:
                logger.error(f"unable to load old component file: {version_entry.key}")
                raise MigrationError(errno.ENOTRECOVERABLE) from None

            m = re.match(r".*/(el\d+)\.clyso.*", old_ver.loc)
            if not m:
                logger.error(f"unable to identify el version for '{version_entry.key}'")
                raise MigrationError(errno.ENOTRECOVERABLE)

            el_version = cast(str, m.group(1))
            if not el_version:
                logger.error(
                    f"empty el version in loc '{old_ver.loc}' for '{version_entry.key}'"
                )
                raise MigrationError(errno.ENOTRECOVERABLE)

            new_ver = ReleaseComponent(
                name=old_ver.name,
                version=old_ver.version,
                sha1=old_ver.sha1,
                versions=[
                    ReleaseComponentVersion(
                        name=old_ver.name,
                        version=old_ver.version,
                        sha1=old_ver.sha1,
                        build_type=BuildType.rpm,
                        os_version=el_version,
                        arch=ArchType.x86_64,
                        repo_url=old_ver.repo_url,
                        artifacts=ReleaseRPMArtifacts(
                            loc=old_ver.loc,
                            release_rpm_loc=old_ver.release_rpm_loc,
                        ),
                    )
                ],
            )

            new_data = new_ver.model_dump_json(indent=2)
            try:
                await s3_upload_json(secrets, version_entry.key, new_data)
            except Exception as e:
                logger.error(
                    f"unable to upload new component JSON to '{version_entry.key}': {e}"
                )
                raise MigrationError(errno.ENOTRECOVERABLE) from e

            logger.info(f"migrated to '{version_entry.key}' to new format")
    pass


async def _migrate_releases_v1(secrets: SecretsVaultMgr) -> None:
    releases_path = "releases/"

    try:
        releases_lst_res = await s3_list(
            secrets, prefix=releases_path, prefix_as_directory=True
        )
    except Exception as e:
        logger.error(f"error listing releases from s3: {e}")
        raise MigrationError(errno.ENOTRECOVERABLE) from e

    logger.info(f"=> migrate {len(releases_lst_res.objects)} releases")
    for rel_entry in releases_lst_res.objects:
        if not rel_entry.key.endswith(".json"):
            logger.warning(f"skipping non-JSON release file: {rel_entry.key}")
            continue

        try:
            data = await s3_download_str_obj(secrets, rel_entry.key, content_type=None)
        except Exception as e:
            logger.error(f"unable to download '{rel_entry.key}': {e}")
            raise MigrationError(errno.ENOTRECOVERABLE) from e

        if not data:
            logger.warning(f"found empty release file: {rel_entry.key}")
            continue

        try:
            _ = ReleaseDesc.model_validate_json(data)
            logger.info(f"already migrated release file: {rel_entry.key}")
            continue
        except pydantic.ValidationError:
            logger.info(f"need to migrate '{rel_entry.key}'")
        except Exception as e:
            logger.error(f"error checking '{rel_entry.key}': {e}")
            raise MigrationError(errno.ENOTRECOVERABLE) from e

        logger.info(f"migrating release file '{rel_entry.key}'")
        try:
            old_ver = ReleaseDescV1.model_validate_json(data)
            logger.info(f"  loaded old version release file: {rel_entry.key}")
        except pydantic.ValidationError:
            logger.error(f"unable to load old release file: {rel_entry.key}")
            raise MigrationError(errno.ENOTRECOVERABLE) from None

        new_components: dict[str, ReleaseComponentVersion] = {}
        for comp_name, comp in old_ver.components.items():
            new_comp = ReleaseComponentVersion(
                name=comp.name,
                version=comp.version,
                sha1=comp.sha1,
                build_type=BuildType.rpm,
                os_version=f"el{old_ver.el_version}",
                arch=ArchType.x86_64,
                repo_url=comp.repo_url,
                artifacts=ReleaseRPMArtifacts(
                    loc=comp.loc,
                    release_rpm_loc=comp.release_rpm_loc,
                ),
            )
            new_components[comp_name] = new_comp

        new_ver = ReleaseDesc(
            version=old_ver.version,
            builds={
                ArchType.x86_64: ReleaseBuildEntry(
                    arch=ArchType.x86_64,
                    build_type=BuildType.rpm,
                    os_version=f"el{old_ver.el_version}",
                    components=new_components,
                )
            },
        )
        new_data = new_ver.model_dump_json(indent=2)
        try:
            await s3_upload_json(secrets, rel_entry.key, new_data)
        except Exception as e:
            logger.error(f"unable to upload new release JSON to '{rel_entry.key}': {e}")
            raise MigrationError(errno.ENOTRECOVERABLE) from e

        logger.info(f"migrated to '{rel_entry.key}' to new format")

    pass


async def migrate_releases_v1(secrets: SecretsVaultMgr) -> None:
    """Migrate releases from v1 to current format."""
    await _migrate_components_v1(secrets)
    await _migrate_releases_v1(secrets)
