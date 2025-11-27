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

from cbscore.containers import ContainerError, find_path_relative_to
from cbscore.containers import logger as parent_logger
from cbscore.containers.desc import ContainerDescriptor, ContainerScript
from cbscore.core.component import CoreComponentLoc
from cbscore.utils.buildah import BuildahContainer, BuildahError
from cbscore.versions.utils import (
    normalize_version,
)

logger = parent_logger.getChild("component")


def _get_container_desc(
    component: CoreComponentLoc,
    version: str,
    *,
    vars: dict[str, Any] | None = None,  # pyright: ignore[reportExplicitAny]
) -> tuple[Path, ContainerDescriptor]:
    if not component.path.exists() or not component.path.is_dir():
        msg = f"component path '{component.path}' does not exist or is not a directory"
        logger.error(msg)
        raise ContainerError(msg)

    ver = normalize_version(version)
    logger.debug(
        f"find container.yaml for '{component.comp.name}' "
        + f"path '{component.path}' ver '{ver}'"
    )

    def _find_container_root_path(loc: Path) -> Path:
        # this is a copy of 'cbscore.versions.utils.parse_version()'s
        # match pattern. We should deduplicate this at some point.
        rs = r"""
            ^
            (?:(?P<prefix>(\w+))-)?
            v?(?P<major>\d+)
            (?:\.(?P<minor>\d+)
                (?:\.(?P<patch>\d+)
                (?:-(?P<suffix>[\w_.-]+))?
                )?
            )?
            $
        """
        rc = re.compile(rs, re.VERBOSE)
        version_m = re.match(rc, ver)
        if not version_m:
            msg = f"unable to parse version '{version}'"
            logger.error(msg)
            raise ContainerError(msg)

        # find all container.yaml files under 'loc', recursively.
        # we will consider the one that matches closest to the provided version.
        # no merge of container.yaml (by precedence) is expected.
        # candidate list is reversed so we can iterate from the leaves first (i.e.,
        # the most specific versions).
        candidates = reversed(list(loc.rglob("container.yaml")))

        best_rank = -1
        best: Path | None = None
        for candidate in candidates:
            if candidate.parent == loc and best_rank < 0:
                best_rank = 0
                best = candidate
                logger.debug(f"found root candidate at '{candidate}'")
                continue

            parent_name = candidate.parent.name
            parent_m = re.match(rc, parent_name)
            if not parent_m:
                logger.warning(f"unexpecter malformed version '{parent_name}'")
                continue

            matched = 0
            for k in ("prefix", "major", "minor", "patch", "suffix"):
                ver_m_val = version_m.group(k)
                parent_m_val = parent_m.group(k)

                if ver_m_val != parent_m_val:
                    # found diverging version part
                    if ver_m_val is not None and parent_m_val is None:
                        # but the divergene is because the parent directory does not
                        # have that bit. hence, the parent is a candidate for this
                        # version.
                        break

                    # it's a mismatch, reset 'matched' to a rank that is ignored.
                    matched = -1
                    break

                matched += 1

            if matched > best_rank:
                logger.debug(
                    f"found preffered candidate, rank '{matched}' loc '{candidate}'"
                )
                best_rank = matched
                best = candidate

        if not best:
            msg = f"unable to find candidate container.yaml under '{loc}'"
            logger.error(msg)
            raise ContainerError(msg)

        logger.debug(f"found candidate container.yaml at '{best}'")
        return best

    containers_path = component.path / component.comp.containers.path
    try:
        yaml_path = _find_container_root_path(containers_path)
    except ContainerError as e:
        msg = f"error finding container.yaml for '{component.path}': {e}"
        logger.error(msg)
        raise ContainerError(msg) from e

    try:
        return (yaml_path, ContainerDescriptor.load(yaml_path, vars=vars))
    except ContainerError as e:
        msg = f"error loading container.yaml from '{yaml_path}': {e}"
        logger.error(msg)
        raise ContainerError(msg) from e


class ComponentContainer:
    version: str
    component_loc: CoreComponentLoc
    container_file_path: Path
    desc: ContainerDescriptor

    def __init__(
        self,
        component_loc: CoreComponentLoc,
        version: str,
        *,
        vars: dict[str, Any] | None = None,  # pyright: ignore[reportExplicitAny]
    ) -> None:
        self.version = version
        self.component_loc = component_loc

        self.container_file_path, self.desc = _get_container_desc(
            component_loc,
            version,
            vars=vars,
        )

    @property
    def _container_path(self) -> Path:
        return self.component_loc.path / self.component_loc.comp.containers.path

    async def apply_pre(self, container: BuildahContainer) -> None:
        # run pre scripts
        #
        for entry in self.desc.pre.scripts:
            try:
                await self._run_script(container, entry)
            except ContainerError as e:
                msg = f"error running PRE script '{entry.name}': {e}"
                logger.error(msg)
                raise ContainerError(msg) from e

        # import keys
        #
        for key in self.desc.pre.keys:
            try:
                cmd = ["rpm", "--import", key]
                await container.run(cmd)
            except (BuildahError, Exception) as e:
                msg = f"error importing key '{key}': {e}"
                logger.error(msg)
                raise ContainerError(msg) from e

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
                    *dnf_packages,
                ]
                await container.run(cmd)
            except (BuildahError, Exception) as e:
                msg = f"error installing PRE packages: {e}"
                logger.error(msg)
                raise ContainerError(msg) from e

        # then install packages from URLs
        #
        for package in https_packages:
            try:
                cmd = ["rpm", "-Uvh", package]
                await container.run(cmd)
            except (BuildahError, Exception) as e:
                msg = f"error installing RPM package '{package}': {e}"
                logger.error(msg)
                raise ContainerError(msg) from e

        # install repositories, if any
        #
        if self.desc.pre.repos:
            for repo in self.desc.pre.repos:
                try:
                    await repo.install(
                        container,
                        self.container_file_path,
                        self._container_path,
                    )
                except (ContainerError, Exception) as e:
                    msg = f"error installing repository '{repo.name}': {e}"
                    logger.error(msg)
                    raise ContainerError(msg) from e

    def get_packages(self, *, optional: bool = False) -> list[str]:
        packages: list[str] = []
        for package_section in self.desc.packages.required:
            lst = package_section.packages
            logger.debug(
                f"get {len(lst)} packages from section '{package_section.section}'"
            )
            packages.extend(lst)

        if optional:
            # TODO: ignore optional for now
            pass

        logger.debug(f"got {len(packages)} packages")
        return packages

    async def _run_script(
        self, container: BuildahContainer, script: ContainerScript
    ) -> None:
        p = find_path_relative_to(
            script.run,
            self.container_file_path,
            self._container_path,
        )
        if not p:
            logger.warning(
                f"unable to find script '{script.run}' for '{script.name}', "
                + f"searched '{self.container_file_path}' and '{self._container_path}'"
            )
            return

        logger.debug(f"run script '{script.name}' from '{p}")
        dest = f"/{p.name}"

        try:
            await container.copy(p, dest)
            await container.run([dest])
            await container.run(["rm", "-f", dest])
        except (BuildahError, Exception) as e:
            msg = f"error running script '{script.name}': {e}"
            logger.error(msg)
            raise ContainerError(msg) from e

    async def apply_post(self, container: BuildahContainer) -> None:
        """Apply post scripts to the container."""
        for entry in self.desc.post:
            try:
                await self._run_script(container, entry)
            except ContainerError as e:
                msg = f"error running POST script '{entry.name}': {e}"
                logger.error(msg)
                raise ContainerError(msg) from e

    async def apply_config(self, container: BuildahContainer) -> None:
        if not self.desc.config:
            logger.debug("no config to apply")
            return

        env: dict[str, str] | None = None
        labels: dict[str, str] | None = None
        annotations: dict[str, str] | None = None

        if len(self.desc.config.env) > 0:
            env = self.desc.config.env
        if len(self.desc.config.labels) > 0:
            labels = self.desc.config.labels
        if len(self.desc.config.annotations) > 0:
            annotations = self.desc.config.annotations

        if not env and not labels and not annotations:
            logger.debug("empty config section")
            return

        await container.set_config(
            env=env,
            labels=labels,
            annotations=annotations,
        )
