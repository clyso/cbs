# CES library - CES container images, component-specific
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


import re
from pathlib import Path
from typing import Any

from ceslib.containers import ContainerError, find_path_relative_to
from ceslib.containers import log as parent_logger
from ceslib.containers.desc import ContainerDescriptor
from ceslib.utils.buildah import BuildahContainer, BuildahError
from ceslib.versions.utils import (
    get_major_version,
    get_minor_version,
    normalize_version,
)

log = parent_logger.getChild("component")


def _get_container_desc(
    component_path: Path,
    version: str,
    *,
    vars: dict[str, Any] | None = None,  # pyright: ignore[reportExplicitAny]
) -> tuple[Path, ContainerDescriptor]:
    def _find_container_yaml() -> Path | None:
        ver = normalize_version(version)
        log.debug(f"find container.yaml for '{ver}'")

        candidates = list(component_path.rglob("container.yaml"))
        log.debug(f"candidates: {candidates}")

        for c in reversed(candidates):
            p = c.parent
            if ver == p.name:
                return c
            elif get_minor_version(ver) == p.name:
                return c
            elif get_major_version(ver) == p.name:
                return c
            elif c.parent == component_path:
                return c
        return None

    container_yaml = _find_container_yaml()
    if not container_yaml:
        msg = (
            f"unable to find container.yaml in '{component_path}' "
            + f"for version '{version}'"
        )
        log.error(msg)
        raise ContainerError(msg)

    log.debug(
        f"found container.yaml for '{component_path}' "
        + f"version '{version}' at '{container_yaml}'"
    )

    try:
        return (container_yaml, ContainerDescriptor.load(container_yaml, vars=vars))
    except ContainerError as e:
        msg = f"error loading container.yaml from '{container_yaml}': {e}"
        log.error(msg)
        raise ContainerError(msg)


class ComponentContainer:
    version: str
    component_path: Path
    raw_container_path: Path
    desc: ContainerDescriptor

    def __init__(
        self,
        component_path: Path,
        version: str,
        *,
        vars: dict[str, Any] | None = None,  # pyright: ignore[reportExplicitAny]
    ) -> None:
        self.version = version
        self.component_path = component_path

        self.raw_container_path, self.desc = _get_container_desc(
            component_path,
            version,
            vars=vars,
        )

    async def apply_pre(self, container: BuildahContainer) -> None:
        # import keys
        #
        for key in self.desc.pre.keys:
            try:
                cmd = ["rpm", "--import", key]
                await container.run(cmd)
            except (BuildahError, Exception) as e:
                msg = f"error importing key '{key}': {e}"
                log.error(msg)
                raise ContainerError(msg)

        # install required packages
        #
        dnf_packages: list[str] = []
        https_packages: list[str] = []
        for package in self.desc.pre.packages:
            if re.match(r"^https?://.+", package):
                https_packages.append(package)
            else:
                dnf_packages.append(package)

        # install dnf specified packages
        #
        if len(dnf_packages) > 0:
            try:
                cmd = [
                    "dnf",
                    "install",
                    "-y",
                    "--setopt=install_weak_deps=False",
                ] + dnf_packages
                await container.run(cmd)
            except (BuildahError, Exception) as e:
                msg = f"error installing PRE packages: {e}"
                log.error(msg)
                raise ContainerError(msg)

        # then install packages from URLs
        #
        for package in https_packages:
            try:
                cmd = ["rpm", "-Uvh", package]
                await container.run(cmd)
            except (BuildahError, Exception) as e:
                msg = f"error installing RPM package '{package}': {e}"
                log.error(msg)
                raise ContainerError(msg)

        # install repositories, if any
        #
        if self.desc.pre.repos:
            for repo in self.desc.pre.repos:
                try:
                    await repo.install(
                        container, self.raw_container_path, self.component_path
                    )
                except (ContainerError, Exception) as e:
                    msg = f"error installing repository '{repo.name}': {e}"
                    log.error(msg)
                    raise ContainerError(msg)
        pass

    def get_packages(self, *, optional: bool = False) -> list[str]:
        packages: list[str] = []
        for package_section in self.desc.packages.required:
            lst = package_section.packages
            log.debug(
                f"get {len(lst)} packages from section '{package_section.section}'"
            )
            packages.extend(lst)

        if optional:
            # TODO: ignore optional for now
            pass

        log.debug(f"got {len(packages)} packages")
        return packages

    async def apply_post(self, container: BuildahContainer) -> None:
        for entry in self.desc.post:
            p = find_path_relative_to(
                entry.run, self.raw_container_path, self.component_path
            )
            if not p:
                log.warning(f"unable to find script for '{entry.name}'")
                continue

            log.debug(f"run script '{entry.name}' from '{p}")
            dest = f"/{p.name}"

            try:
                await container.copy(p, dest)
                await container.run([dest])
                await container.run(["rm", "-f", dest])
            except (BuildahError, Exception) as e:
                msg = f"error running script '{entry.name}': {e}"
                log.error(msg)
                raise ContainerError(msg)

        pass
