# CES library - CES builder
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

from pathlib import Path

from ceslib.builder import BuilderError
from ceslib.builder import log as parent_logger
from ceslib.builder.prepare import prepare
from ceslib.builder.release import release_desc_build, release_desc_upload
from ceslib.builder.rpmbuild import build_rpms
from ceslib.builder.signing import sign_rpms
from ceslib.builder.upload import s3_upload_rpms
from ceslib.containers import ContainerError
from ceslib.containers.build import ContainerBuilder
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.utils.vault import VaultError
from ceslib.versions.desc import VersionDescriptor

log = parent_logger.getChild("builder")


class Builder:
    desc: VersionDescriptor
    scratch_path: Path
    components_path: Path
    containers_path: Path
    upload: bool
    secrets: SecretsVaultMgr
    ccache_path: Path | None
    skip_build: bool

    def __init__(
        self,
        desc: VersionDescriptor,
        vault_addr: str,
        vault_role_id: str,
        vault_secret_id: str,
        scratch_path: Path,
        secrets_path: Path,
        components_path: Path,
        containers_path: Path,
        *,
        ccache_path: Path | None = None,
        upload: bool = True,
        skip_build: bool = False,
    ) -> None:
        self.desc = desc
        self.scratch_path = scratch_path
        self.components_path = components_path
        self.containers_path = containers_path
        self.upload = upload
        self.ccache_path = ccache_path
        self.skip_build = skip_build

        try:
            self.secrets = SecretsVaultMgr(
                secrets_path, vault_addr, vault_role_id, vault_secret_id
            )
        except VaultError as e:
            log.error(f"error logging in to vault: {e}")
            raise BuilderError(f"error logging in to vault: {e}")

    async def run(self) -> None:
        log.info("preparing builder")

        try:
            components = await prepare(
                self.desc, self.secrets, self.scratch_path, self.components_path
            )
        except BuilderError as e:
            raise BuilderError(f"error preparing builder: {e}")

        log.info("build RPMs")
        rpms_path = self.scratch_path.joinpath("rpms")
        rpms_path.mkdir(exist_ok=True)

        comp_rpms_paths = await build_rpms(
            rpms_path,
            self.desc.el_version,
            self.components_path,
            components,
            ccache_path=self.ccache_path,
            skip_build=self.skip_build,
        )

        log.info("sign RPMs")
        try:
            await sign_rpms(self.secrets, comp_rpms_paths)
        except BuilderError as e:
            msg = f"error signing component RPMs: {e}"
            log.error(msg)
            raise BuilderError(msg)
        except Exception as e:
            msg = f"unknown error signing component RPMs: {e}"
            log.error(msg)
            raise BuilderError(msg)

        if not self.upload:
            return

        log.info(f"upload RPMs: {self.upload}")
        try:
            s3_comp_loc = await s3_upload_rpms(
                self.secrets, comp_rpms_paths, self.desc.el_version
            )
        except BuilderError as e:
            msg = f"error uploading RPMs to S3: {e}"
            log.error(msg)
            raise BuilderError(msg)
        except Exception as e:
            msg = f"unknown error uploading RPMs to S3: {e}"
            log.error(msg)
            raise BuilderError(msg)

        log.info(f"create release descriptor for '{self.desc.version}'")
        release_desc = await release_desc_build(
            self.desc, self.components_path, s3_comp_loc
        )

        try:
            await release_desc_upload(self.secrets, release_desc)
        except BuilderError as e:
            msg = (
                "error uploading release descriptor for version "
                + f"'{release_desc.version}' to S3: {e}"
            )
            log.error(msg)
            raise BuilderError(msg)

        try:
            ctr_builder = ContainerBuilder(
                self.desc, release_desc, self.containers_path
            )
            await ctr_builder.build()
        except (ContainerError, Exception) as e:
            msg = f"error creating container: {e}"
            log.error(msg)
            raise BuilderError(msg)

    pass
