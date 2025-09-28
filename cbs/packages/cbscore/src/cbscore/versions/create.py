# CES library - version descriptor creation
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

import enum
import re
from typing import cast

from cbscore.logger import logger as parent_logger
from cbscore.versions.desc import (
    VersionComponent,
    VersionDescriptor,
    VersionImage,
    VersionSignedOffBy,
)
from cbscore.versions.errors import VersionError

logger = parent_logger.getChild("versions_create")


class _Prios(enum.Enum):
    GA = 0
    RC = 1
    DEV = 2
    TEST = 3
    CI = 4


class VersionType(enum.Enum):
    RELEASE = 1
    TESTING = 2


_release_types: dict[str, tuple[int, str]] = {
    "ga": (_Prios.GA.value, "General Availability"),
    "rc": (_Prios.RC.value, "Release Candidate"),
    "dev": (_Prios.DEV.value, "Development release"),
    "test": (_Prios.TEST.value, "Test release"),
    "ci": (_Prios.CI.value, "CI/CD release"),
}

component_repos: dict[str, str] = {
    "ceph": "https://github.com/clyso/ceph",
}


def _validate_version(v: str) -> bool:
    return re.match(r"^.*v\d{2}\.\d+\.\d+$", v) is not None


def _obtain_type(s: str) -> tuple[str, int] | None:
    regex = r"^([a-z]+)=(\d+)$"
    m = re.match(regex, s)
    if m is None:
        return None
    return (m.group(1), cast(int, m.group(2)))


def _parse_types(version_types: list[str]) -> list[tuple[str, int]]:
    types_lst: list[tuple[str, int]] = []

    highest_index: int = -1
    found_types: list[str] = []

    for t in version_types:
        res = _obtain_type(t)
        if res is None:
            msg = f"malformed version type '{t}'"
            logger.error(msg)
            raise VersionError(msg)

        rel_entry = _release_types.get(res[0], None)
        if rel_entry is None:
            msg = f"unknown version type '{res[0]}'"
            logger.error(msg)
            raise VersionError(msg)

        rel_idx, _ = rel_entry
        if highest_index > rel_idx:
            msg = f"malformed type sequence: '{res[0]}' must come after '{types_lst}'"
            logger.error(msg)
            raise VersionError(msg)
        highest_index = rel_idx

        if res[0] in found_types:
            msg = f"multiple types '{res[0]}' found in provided types"
            logger.error(msg)
            raise VersionError(msg)
        found_types.append(res[0])

        types_lst.append((res[0], res[1]))

    return types_lst


def _parse_components(components: list[str]) -> dict[str, str]:
    comps: dict[str, str] = {}

    for c in components:
        m = re.match(r"^([\w_-]+)@([\d\w_.-]+)$", c)
        if not m:
            msg = f"malformed component name/version pair '{c}'"
            logger.error(msg)
            raise VersionError(msg)
        comps[m.group(1)] = m.group(2)

    return comps


def _parse_component_overrides(overrides: list[str]) -> dict[str, str]:
    override_map: dict[str, str] = {}

    regex = re.compile(r"^([\w_-]+)=([\d\w_.:/-]+)$")
    for override in overrides:
        m = re.match(regex, override)
        if not m:
            msg = f"malformed component override '{override}'"
            logger.error(msg)
            raise VersionError(msg)
        override_map[m.group(1)] = m.group(2)

    return override_map


def _get_version_type(types_lst: list[tuple[str, int]]) -> VersionType:
    if len(types_lst) == 0:
        return VersionType.RELEASE

    what: VersionType | None = None
    for t, _ in types_lst:
        assert t in _release_types
        if what is not None:
            what = VersionType.TESTING
            break

        if _release_types[t][0] <= _Prios.RC.value:
            what = VersionType.RELEASE
        else:
            what = VersionType.TESTING
            break

    assert what is not None
    return what


def create(
    version: str,
    version_types: list[str],
    components: list[str],
    component_overrides: list[str],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
    user_name: str,
    user_email: str,
) -> tuple[VersionType, VersionDescriptor]:
    if not _validate_version(version):
        msg = f"malformed version '{version}'"
        logger.error(msg)
        raise VersionError(msg)

    types_lst = _parse_types(version_types)
    version_type = _get_version_type(types_lst)

    components_map = _parse_components(components)
    if len(components_map) == 0:
        msg = "missing valid components"
        logger.error(msg)
        raise VersionError(msg)

    for c in components_map:
        if c not in component_repos:
            msg = f"unknown component '{c}' specified"
            logger.error(msg)
            raise VersionError(msg)

    component_overrides_map = _parse_component_overrides(component_overrides)
    for c in component_overrides_map:
        if c not in components_map:
            msg = f"missing component '{c}' for override"
            logger.error(msg)
            raise VersionError(msg)

    version_types_str = "-".join([f"{t}.{n}" for t, n in types_lst])
    version_str = f"{version}" + (f"-{version_types_str}" if version_types_str else "")
    version_types_title = " ".join(
        [f"{_release_types[t][1]} #{n}" for t, n in types_lst]
    )
    version_title = f"Release {version}" + (
        f", {version_types_title}" if version_types_title else ""
    )

    logger.debug(f"version types: {version_types_str}")
    logger.debug(f"version str: {version_str}")
    logger.debug(f"version types title: {version_types_title}")
    logger.debug(f"version title: {version_title}")

    component_res: list[VersionComponent] = []
    for comp_name, comp_version in components_map.items():
        comp_repo = component_repos[comp_name]
        if comp_name in component_overrides_map:
            comp_repo = component_overrides_map[comp_name]

        component_res.append(
            VersionComponent(name=comp_name, repo=comp_repo, ref=comp_version)
        )

    image_tag_str = image_tag if image_tag else version_str

    desc = VersionDescriptor(
        version=version_str,
        title=version_title,
        signed_off_by=VersionSignedOffBy(
            user=user_name,
            email=user_email,
        ),
        image=VersionImage(
            registry=registry,
            name=image_name,
            tag=image_tag_str,
        ),
        components=component_res,
        distro=distro,
        el_version=el_version,
    )

    return (version_type, desc)
