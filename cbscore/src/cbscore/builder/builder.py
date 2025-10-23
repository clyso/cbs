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
from cbscore.config import VaultConfig
from cbscore.containers import ContainerError
from cbscore.containers.build import ContainerBuilder
from cbscore.core.component import CoreComponentLoc, load_components
from cbscore.images.skopeo import skopeo_image_exists
from cbscore.releases import ReleaseError
from cbscore.releases.desc import (
    ArchType,
    BuildType,
    ReleaseBuildEntry,
    ReleaseComponent,
    ReleaseComponentVersion,
    ReleaseDesc,
    ReleaseRPMArtifacts,
)
from cbscore.releases.s3 import (
    check_release_exists,
    check_released_components,
    release_desc_upload,
    release_upload_components,
)
from cbscore.releases.utils import get_component_release_rpm
from cbscore.utils.containers import get_container_canonical_uri
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
        vault_config: VaultConfig,
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
            self.secrets = SecretsVaultMgr(secrets_path, vault_config)
        except VaultError as e:
            msg = f"error logging in to vault: {e}"
            logger.error(msg)
            raise BuilderError(msg) from e

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
            logger.error(msg)
            raise BuilderError(msg=msg) from e

        container_img_uri = get_container_canonical_uri(self.desc)
        if skopeo_image_exists(container_img_uri, self.secrets):
            logger.info(f"image '{container_img_uri}' already exists -- do not build!")
            return

        release_desc = await check_release_exists(self.secrets, self.desc.version)

        # FIXME: checking for arch must be done agaisnt the version descriptor,
        # instead of hardcoded.
        if release_desc and release_desc.builds.get(ArchType.x86_64):
            if not self.force:
                logger.info(
                    f"found existing x86_64 release for version '{self.desc.version}', "
                    + "not building"
                )
                return
            else:
                logger.warning(
                    "force flag set, rebuild existing x86_64 release"
                    + f"'{self.desc.version}'"
                )

        try:
            release_desc = await self._build_release()
        except (BuilderError, Exception) as e:
            msg = f"error building components: {e}"
            logger.error(msg)
            raise BuilderError(msg) from e

        if not release_desc:
            if self.upload:
                # this should not happen!
                msg = "unexpected missing release descriptor!"
                logger.error(msg)
                raise BuilderError(msg)

            logger.warning("not uploading, build done")
            return

        try:
            ctr_builder = ContainerBuilder(self.desc, release_desc, self.components)
            await ctr_builder.build()
            await ctr_builder.finish(self.secrets)
        except (ContainerError, Exception) as e:
            msg = f"error creating container: {e}"
            logger.error(msg)
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
        existing: dict[str, ReleaseComponentVersion] = {}

        if not self.force:
            try:
                to_check = {
                    comp.name: comp.long_version for comp in components.values()
                }
                found = await check_released_components(self.secrets, to_check)
            except (BuilderError, Exception) as e:
                msg = f"error checking released components: {e}"
                logger.exception(msg)
                raise BuilderError(msg) from e

            # from the found components, do they match the required architecture,
            # build type, and os version?
            #
            for comp_name, comp_rel in found.items():
                for ver in comp_rel.versions:
                    if (
                        ver.arch == ArchType.x86_64
                        and ver.build_type == BuildType.rpm
                        and ver.os_version == f"el{self.desc.el_version}"
                    ):
                        existing[comp_name] = ver
                        break

        to_build = {
            name: info for name, info in components.items() if name not in existing
        }

        built: dict[str, ReleaseComponentVersion] = {}
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

        comp_versions = existing.copy()
        comp_versions.update(built)

        if not comp_versions:
            msg = (
                f"no component release versions found, existing: {existing.keys()}, "
                + f"built: {built.keys()}"
            )
            logger.error(msg)
            raise BuilderError(msg)

        release_build = ReleaseBuildEntry(
            arch=ArchType.x86_64,
            build_type=BuildType.rpm,
            os_version=f"el{self.desc.el_version}",
            components=comp_versions,
        )

        # release = ReleaseDesc(
        #     version=self.desc.version,
        #     el_version=self.desc.el_version,
        #     components=comp_releases,
        # )

        try:
            release = await release_desc_upload(
                self.secrets, self.desc.version, release_build
            )
        except (BuilderError, Exception) as e:
            msg = f"error uploading release desc to S3: {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        return release

    async def _build(
        self, components: dict[str, BuildComponentInfo]
    ) -> dict[str, ReleaseComponentVersion]:
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
            comp_versions = await self._upload(components, comp_builds)
        except (BuilderError, Exception) as e:
            msg = f"error uploading component builds '{comp_builds.keys()}': {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        return comp_versions

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
    ) -> dict[str, ReleaseComponentVersion]:
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

        # obtain existing released components at their versions.
        #
        comp_versions = {name: info.long_version for name, info in comp_infos.items()}
        try:
            existing_components = await check_released_components(
                self.secrets, comp_versions
            )
        except ReleaseError as e:
            msg = f"error checking existing released components: {e}"
            logger.error(msg)
            raise BuilderError(msg) from e
        except Exception as e:
            msg = f"unknown error checking existing released components: {e}"
            logger.error(msg)
            raise BuilderError(msg) from e

        # create individual component's release descriptors, which will then
        # be returned.
        #
        comp_releases: dict[str, ReleaseComponent] = {}
        comp_rel_versions: dict[str, ReleaseComponentVersion] = {}
        for name, infos in comp_infos.items():
            if name not in s3_comp_loc:
                msg = f"unexpected missing component '{name}' in S3 upload result"
                logger.error(msg)
                raise BuilderError(msg)

            if name not in comp_builds:
                msg = f"unexpected missing component '{name}' in builds"
                logger.error(msg)
                raise BuilderError(msg)

            comp_release: ReleaseComponent = (
                existing_components[name]
                if name in existing_components
                else ReleaseComponent(
                    name=name, version=infos.long_version, sha1=infos.sha1, versions=[]
                )
            )

            release_comp_s3_loc = s3_comp_loc[name].location
            release_rpm_loc = await get_component_release_rpm(
                self.components[name], self.desc.el_version
            )
            if not release_rpm_loc:
                logger.error(
                    "unable to find component release RPM location "
                    + f"for '{name}' el version '{self.desc.el_version}' -- "
                    + "ignore component"
                )
                continue

            release_rpm_s3_loc = f"{release_comp_s3_loc}/{release_rpm_loc}"

            comp_release_ver = ReleaseComponentVersion(
                name=name,
                version=infos.long_version,
                sha1=infos.sha1,
                arch=ArchType.x86_64,
                build_type=BuildType.rpm,
                os_version=f"el{self.desc.el_version}",
                repo_url=infos.repo_url,
                artifacts=ReleaseRPMArtifacts(
                    loc=release_comp_s3_loc,
                    release_rpm_loc=release_rpm_s3_loc,
                ),
            )
            comp_release.versions.append(comp_release_ver)
            comp_rel_versions[name] = comp_release_ver

            # comp_release = await release_component_desc(
            #     component_loc=self.components[name],
            #     component_name=name,
            #     repo_url=infos.repo_url,
            #     long_version=infos.long_version,
            #     sha1=infos.sha1,
            #     s3_location=s3_comp_loc[name].location,
            #     build_el_version=self.desc.el_version,
            # )
            # if not comp_release:
            #     logger.error(
            #         f"unable to obtain component '{name}' "
            #         + "release descriptor, ignore"
            #     )
            #     continue

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

        return comp_rel_versions
