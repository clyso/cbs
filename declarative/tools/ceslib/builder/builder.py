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
from ceslib.builder.prepare import prepare_builder, prepare_components
from ceslib.builder.release import (
    ReleaseDesc,
    check_release_exists,
    release_desc_build,
    release_desc_upload,
)
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
    force: bool

    def __init__(
        self,
        desc: VersionDescriptor,
        vault_addr: str,
        vault_role_id: str,
        vault_secret_id: str,
        vault_transit: str,
        scratch_path: Path,
        secrets_path: Path,
        components_path: Path,
        containers_path: Path,
        *,
        ccache_path: Path | None = None,
        upload: bool = True,
        skip_build: bool = False,
        force: bool = False,
    ) -> None:
        self.desc = desc
        self.scratch_path = scratch_path
        self.components_path = components_path
        self.containers_path = containers_path
        self.upload = upload
        self.ccache_path = ccache_path
        self.skip_build = skip_build
        self.force = force

        try:
            self.secrets = SecretsVaultMgr(
                secrets_path,
                vault_addr,
                vault_role_id,
                vault_secret_id,
                vault_transit=vault_transit,
            )
        except VaultError as e:
            log.error(f"error logging in to vault: {e}")
            raise BuilderError(f"error logging in to vault: {e}")

    async def run(self) -> None:
        log.info("preparing builder")
        try:
            await prepare_builder()
        except BuilderError as e:
            msg = f"error preparing builder: {e}"
            log.error(msg)
            raise BuilderError(msg)

        release_desc = await check_release_exists(self.secrets, self.desc.version)
        if not release_desc or self.force:
            try:
                release_desc = await self._build()
            except (BuilderError, Exception) as e:
                msg = f"error building components: {e}"
                log.error(msg)
                raise BuilderError(msg)

            if not release_desc:
                if self.upload:
                    msg = "unexpected error building components"
                    log.error(msg)
                    raise BuilderError(msg)

                log.warning("not uploading, finish build")
                return
        else:
            log.info(
                f"found existing release for version '{self.desc.version}', not building"
            )

        try:
            ctr_builder = ContainerBuilder(
                self.desc, release_desc, self.containers_path
            )
            await ctr_builder.build()
            await ctr_builder.finish(self.secrets)
        except (ContainerError, Exception) as e:
            msg = f"error creating container: {e}"
            log.error(msg)
            raise BuilderError(msg)

    pass

    async def _build(self) -> ReleaseDesc | None:
        log.info(f"prepare components for version '{self.desc.version}'")
        try:
            components = await prepare_components(
                self.secrets,
                self.scratch_path,
                self.components_path,
                self.desc.components,
                self.desc.version,
            )
        except BuilderError as e:
            msg = f"error preparing components: {e}"
            log.error(msg)
            raise BuilderError(msg)

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
            return None

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
            self.desc, components, self.components_path, s3_comp_loc
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

        return release_desc
