# CES library - CES container images, build
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

from ceslib.builder import log as parent_logger
from ceslib.builder.release import ReleaseDesc
from ceslib.containers import ContainerError
from ceslib.containers.component import ComponentContainer
from ceslib.utils.buildah import BuildahContainer, BuildahError, buildah_new_container
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.versions.desc import VersionDescriptor

log = parent_logger.getChild("containers")


def _get_component_path(containers_path: Path, component: str) -> Path | None:
    p = containers_path.joinpath(component)
    if not p.exists() or not p.is_dir():
        log.warning(f"container path does not exist for component '{component}'")
        return None
    return p


class ContainerBuilder:
    version_desc: VersionDescriptor
    release_desc: ReleaseDesc
    containers_path: Path

    container: BuildahContainer | None

    def __init__(
        self,
        version_desc: VersionDescriptor,
        release_desc: ReleaseDesc,
        containers_path: Path,
    ) -> None:
        self.version_desc = version_desc
        self.release_desc = release_desc
        self.containers_path = containers_path
        self.container = None

    async def build(self) -> None:
        try:
            components = await self.get_components()
        except (ContainerError, Exception) as e:
            msg = f"error obtaining components to build: {e}"
            log.error(msg)
            raise ContainerError(msg)

        self.container = await buildah_new_container(self.version_desc)

        try:
            await self.apply_pre(components)
        except (ContainerError, Exception) as e:
            msg = f"error applying PRE sections: {e}"
            log.error(msg)
            raise ContainerError(msg)

        try:
            await self.install_packages(components)
        except (ContainerError, Exception) as e:
            msg = f"error installing component PACKAGES: {e}"
            log.error(msg)
            raise ContainerError(msg)

        try:
            await self.apply_post(components)
        except (ContainerError, Exception) as e:
            msg = f"error applying POST sections: {e}"
            log.error(msg)
            raise ContainerError(msg)

        pass

    async def get_components(
        self,
    ) -> dict[str, ComponentContainer]:
        log.info(f"build container for '{self.version_desc.version}'")

        components: dict[str, ComponentContainer] = {}

        for component in self.version_desc.components:
            comp_name = component.name
            comp_path = _get_component_path(self.containers_path, comp_name)
            if not comp_path:
                log.warning(
                    f"unable to find container path for '{comp_name}', skipping"
                )
                continue

            if comp_name not in self.release_desc.components:
                log.warning(f"component '{comp_name}' not in release descriptor")
                continue

            release_comp = self.release_desc.components[comp_name]

            vars = {
                "version": release_comp.version,
                "el": self.version_desc.el_version,
            }

            try:
                component = ComponentContainer(
                    comp_path, release_comp.version, vars=vars
                )
            except ContainerError as e:
                msg = (
                    "unable to obtain container's component descriptor "
                    + f"for '{comp_name}': {e}"
                )
                log.error(msg)
                raise ContainerError(msg)
            except Exception as e:
                msg = (
                    "unknown exception obtaining container's component descriptor for "
                    + f"'{comp_name}: {e}"
                )
                log.error(msg)
                raise ContainerError(msg)

            components[comp_name] = component

        if len(components) == 0:
            msg = "no container descriptors found"
            log.error(msg)
            raise ContainerError(msg)

        return components

    async def apply_pre(self, components: dict[str, ComponentContainer]) -> None:
        log.info("apply PRE from components")
        assert self.container

        for comp_name, comp_container in components.items():
            log.info(f"apply PRE for component '{comp_name}'")
            try:
                await comp_container.apply_pre(self.container)
            except (ContainerError, Exception) as e:
                msg = f"error applying PRE to component '{comp_name}': {e}"
                log.error(msg)
                raise ContainerError(msg)
        pass

    def get_packages(self, components: dict[str, ComponentContainer]) -> list[str]:
        packages: list[str] = []
        log.info("get packages from components")
        for comp_name, comp_container in components.items():
            log.info(f"get packages for component '{comp_name}'")
            # TODO: ignore optional packages for now
            packages.extend(comp_container.get_packages(optional=False))

        return packages

    async def install_packages(self, components: dict[str, ComponentContainer]) -> None:
        log.info("install PACKAGES")
        assert self.container

        # obtain packages from all components, install all in one go
        packages = self.get_packages(components)
        if len(packages) == 0:
            log.info("no packages to install")
            return

        try:
            cmd = [
                "dnf",
                "install",
                "-y",
                "--setopt=install_weak_deps=False",
                "--setopt=skip_missing_names_on_install=False",
                "--enablerepo=crb",
            ] + packages

            await self.container.run(cmd)
        except (BuildahError, Exception) as e:
            msg = f"error installing packages: {e}"
            log.error(msg)
            raise ContainerError(msg)

    async def apply_post(self, components: dict[str, ComponentContainer]) -> None:
        log.info("apply POST from components")
        assert self.container

        log.info("run final container update")
        try:
            cmd = ["dnf", "update", "-y"]
            await self.container.run(cmd)
        except (BuildahError, Exception) as e:
            msg = f"error running final container update: {e}"
            log.error(msg)
            raise ContainerError(msg)

        for comp_name, comp_container in components.items():
            log.info(f"apply POST for component '{comp_name}'")
            await comp_container.apply_post(self.container)

        pass

    async def finish(self, secrets: SecretsVaultMgr) -> None:
        log.info(f"finish container for '{self.version_desc.version}'")
        assert self.container
        await self.container.finish(secrets)
