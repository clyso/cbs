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

from cbscore.builder import BuilderError
from cbscore.builder import logger as parent_logger
from cbscore.builder.prepare import (
    BuildComponentInfo,
    prepare_builder,
    prepare_components,
)
from cbscore.builder.rpmbuild import ComponentBuild, build_rpms
from cbscore.builder.signing import sign_rpms
from cbscore.builder.upload import s3_upload_rpms
from cbscore.containers import ContainerError
from cbscore.containers.build import ContainerBuilder
from cbscore.core.component import CoreComponentLoc, load_components
from cbscore.releases.desc import (
    ReleaseComponent,
    ReleaseDesc,
    release_component_desc,
)
from cbscore.releases.s3 import (
    check_release_exists,
    check_released_components,
    release_desc_upload,
    release_upload_components,
)
from cbscore.utils.secrets import SecretsVaultMgr
from cbscore.utils.vault import VaultError
from cbscore.versions.desc import VersionDescriptor

logger = parent_logger.getChild("builder")


class Builder:
    desc: VersionDescriptor
    scratch_path: Path
    components: dict[str, CoreComponentLoc]
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
        *,
        ccache_path: Path | None = None,
        upload: bool = True,
        skip_build: bool = False,
        force: bool = False,
    ) -> None:
        self.desc = desc
        self.scratch_path = scratch_path
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
            logger.exception("error logging in to vault")
            raise BuilderError(msg=f"error logging in to vault: {e}") from e

        self.components = load_components(components_path)
        if not self.components:
            msg = f"no components found in '{components_path}'"
            logger.error(msg)
            raise BuilderError(msg)

    async def run(self) -> None:
        logger.info("preparing builder")
        try:
            await prepare_builder()
        except BuilderError as e:
            msg = f"error preparing builder: {e}"
            logger.exception(msg)
            raise BuilderError(msg=msg) from e

        release_desc = await check_release_exists(self.secrets, self.desc.version)
        if not release_desc or self.force:
            try:
                release_desc = await self._build_release()
            except (BuilderError, Exception) as e:
                msg = f"error building components: {e}"
                logger.exception(msg)
                raise BuilderError(msg) from e

            if not release_desc:
                if self.upload:
                    # this should not happen!
                    msg = "unexpected missing release descriptor!"
                    logger.error(msg)
                    raise BuilderError(msg)

                logger.warning("not uploading, build done")
                return

        else:
            logger.info(
                f"found existing release for version '{self.desc.version}', "
                + "not building"
            )

        try:
            ctr_builder = ContainerBuilder(self.desc, release_desc, self.components)
            await ctr_builder.build()
            await ctr_builder.finish(self.secrets)
        except (ContainerError, Exception) as e:
            msg = f"error creating container: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

    pass

    async def _build_release(self) -> ReleaseDesc | None:
        """
        Build a release, returning a `ReleaseDesc`.

        This function will first prepare the builder, assess which components need to
        be built and which already exist in S3, and then build (and sign) those that
        can't be found otherwise.

        Returns a `ReleaseDesc`, composed of all the components that belong to the
        wanted version, composing it from the already existing components (if any) and
        the built components (if any needed to be built).

        Will return `None` if `self.upload` is `False`.
        """
        logger.info(f"build release for '{self.desc.version}'")
        logger.info(f"prepare components for version '{self.desc.version}'")
        try:
            components = await prepare_components(
                self.secrets,
                self.scratch_path,
                self.components,
                self.desc.components,
                self.desc.version,
            )
        except BuilderError as e:
            msg = f"error preparing components: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        # Check if any of the components have been previously built, and, if so,
        # reuse them instead of building them.
        #
        # If the 'force' flag has been set, assume we have no existing components,
        # and force all components to be built.
        #
        existing: dict[str, ReleaseComponent] = {}

        if not self.force:
            try:
                to_check = {
                    comp.name: comp.long_version for comp in components.values()
                }
                existing = await check_released_components(self.secrets, to_check)
            except (BuilderError, Exception) as e:
                msg = f"error checking released components: {e}"
                logger.exception(msg)
                raise BuilderError(msg) from e

        to_build = {
            name: info for name, info in components.items() if name not in existing
        }

        built: dict[str, ReleaseComponent] = {}
        if to_build:
            # build RPMs for required components
            try:
                built = await self._build(to_build)
            except (BuilderError, Exception) as e:
                msg = f"error building components '{to_build.keys()}': {e}"
                logger.exception(msg)
                raise BuilderError(msg) from e

        if not self.upload:
            logger.warning("not uploading per config, stop release build")
            return None

        comp_releases = existing.copy()
        comp_releases.update(built)

        if not comp_releases:
            msg = (
                f"no component releases found, existing: {existing.keys()}, "
                + f"built: {built.keys()}"
            )
            logger.error(msg)
            raise BuilderError(msg)

        release = ReleaseDesc(
            version=self.desc.version,
            el_version=self.desc.el_version,
            components=comp_releases,
        )

        try:
            await release_desc_upload(self.secrets, release)
        except (BuilderError, Exception) as e:
            msg = f"error uploading release desc to S3: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        return release

    async def _build(
        self, components: dict[str, BuildComponentInfo]
    ) -> dict[str, ReleaseComponent]:
        """
        Build all the specified components, sign them, and upload them to S3 (unless the
        `upload` flag is `False`).

        Returns a dict of component names to `ReleaseComponent`, representing
        each finished build that has been uploaded to S3.
        """  # noqa: D205
        logger.debug(f"build components '{components.keys()}")

        if not components:
            logger.info("no components to build")
            return {}

        try:
            comp_builds = await self._build_rpms(components)
        except (BuilderError, Exception) as e:
            msg = f"error building RPMs for '{components.keys()}: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        if not self.upload:
            return {}

        try:
            comp_descs = await self._upload(components, comp_builds)
        except (BuilderError, Exception) as e:
            msg = f"error uploading component builds '{comp_builds.keys()}': {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        return comp_descs

    async def _build_rpms(
        self, components: dict[str, BuildComponentInfo]
    ) -> dict[str, ComponentBuild]:
        """
        Build, sign, and upload components specified in the `components` `dict`.

        Returns a `dict` of component names to their S3 location.
        """
        logger.info(f"build RPMs for components '{components.keys()}'")

        if not components:
            logger.info("no components to build RPMs for, return")
            return {}

        rpms_path = self.scratch_path.joinpath("rpms")
        rpms_path.mkdir(exist_ok=True)

        try:
            comp_builds = await build_rpms(
                rpms_path,
                self.desc.el_version,
                self.components,
                components,
                ccache_path=self.ccache_path,
                skip_build=self.skip_build,
            )
        except (BuilderError, Exception) as e:
            msg = f"error building components ({components.keys()}): {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        logger.info("sign RPMs")
        try:
            await sign_rpms(self.secrets, comp_builds)
        except BuilderError as e:
            msg = f"error signing component RPMs: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e
        except Exception as e:
            msg = f"unknown error signing component RPMs: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        return comp_builds

    async def _upload(
        self,
        comp_infos: dict[str, BuildComponentInfo],
        comp_builds: dict[str, ComponentBuild],
    ) -> dict[str, ReleaseComponent]:
        """
        Upload the provided component builds to S3, along with a component release
        descriptor.

        Returns a dict of component names to their corresponding component release
        descriptor.
        """  # noqa: D205
        logger.info(f"upload RPMs: {self.upload}, components: {comp_builds.keys()}")
        if not self.upload:
            return {}

        if not comp_builds:
            msg = "unexpected empty 'components' builds dict, can't upload"
            logger.error(msg)
            raise BuilderError(msg)

        if not comp_infos:
            msg = "unexpected empty 'components' infos dict, can't upload"
            logger.error(msg)
            raise BuilderError(msg)

        try:
            s3_comp_loc = await s3_upload_rpms(
                self.secrets, comp_builds, self.desc.el_version
            )
        except (BuilderError, Exception) as e:
            msg = f"error uploading RPMs to S3: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        # create individual component's release descriptors, which will then
        # be returned.
        #
        comp_releases: dict[str, ReleaseComponent] = {}
        for name, infos in comp_infos.items():
            if name not in s3_comp_loc:
                msg = f"unexpected missing component '{name}' in S3 upload result"
                logger.error(msg)
                raise BuilderError(msg)

            if name not in comp_builds:
                msg = f"unexpected missing component '{name}' in builds"
                logger.error(msg)
                raise BuilderError(msg)

            comp_release = await release_component_desc(
                component_loc=self.components[name],
                component_name=name,
                repo_url=infos.repo_url,
                long_version=infos.long_version,
                sha1=infos.sha1,
                s3_location=s3_comp_loc[name].location,
                build_el_version=self.desc.el_version,
            )
            if not comp_release:
                logger.error(
                    f"unable to obtain component '{name}' "
                    + "release descriptor, ignore"
                )
                continue

            comp_releases[name] = comp_release

        # Upload the components' release descriptors. This operation will be performed
        # in parallel, hence why we are doing it outside of the loop above.
        #
        try:
            await release_upload_components(self.secrets, comp_releases)
        except (BuilderError, Exception) as e:
            msg = f"error uploading release descriptors for components: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        return comp_releases
