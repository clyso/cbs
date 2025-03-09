#!/usr/bin/env python3

# Handles CES declarative versions
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
import errno
import logging
import re
import sys
from typing import cast

import click
from ceslib.versions.desc import (
    VersionComponent,
    VersionDescriptor,
    VersionImage,
    VersionSignedOffBy,
)
from ceslib.errors import CESError, NoSuchVersionError
from ceslib.images.desc import get_image_desc
from ceslib.logging import log as root_logger
from ceslib.utils.git import get_git_repo_root, get_git_user

log = root_logger.getChild("builds")


class Prios(enum.Enum):
    GA = 0
    RC = 1
    DEV = 2
    TEST = 3
    CI = 4


class BuildType(enum.Enum):
    RELEASE = 1
    TESTING = 2


release_types: dict[str, tuple[int, str]] = {
    "ga": (Prios.GA.value, "General Availability"),
    "rc": (Prios.RC.value, "Release Candidate"),
    "dev": (Prios.DEV.value, "Development release"),
    "test": (Prios.TEST.value, "Test release"),
    "ci": (Prios.CI.value, "CI/CD release"),
}

component_repos: dict[str, str] = {
    "ceph": "https://github.com/clyso/ceph",
    "clyso-mgr": "https://gitlab.clyso.com/ceph/clyso",
    "copilot": "https://gitlab.clyso.com/ceph/ceph-copilot",
}


@click.group()
@click.option("-d", "--debug", envvar="CES_TOOL_DEBUG", is_flag=True)
def main(debug: bool) -> None:
    if debug:
        root_logger.setLevel(logging.DEBUG)
    pass


def _validate_version(v: str) -> bool:
    return re.match(r"^.*v\d{2}\.\d+\.\d+$", v) is not None


def _obtain_type(s: str) -> tuple[str, int] | None:
    regex = r"^([a-z]+)=(\d+)$"
    m = re.match(regex, s)
    if m is None:
        return None
    return (m.group(1), cast(int, m.group(2)))


def _parse_types(build_types: list[str]) -> list[tuple[str, int]]:
    types_lst: list[tuple[str, int]] = []

    highest_index: int = -1
    found_types: list[str] = []

    for t in build_types:
        res = _obtain_type(t)
        if res is None:
            log.error(f"malformed build type '{t}'")
            sys.exit(errno.EINVAL)

        rel_entry = release_types.get(res[0], None)
        if rel_entry is None:
            log.error(f"unknown build type '{res[0]}'")
            sys.exit(errno.EINVAL)

        rel_idx, _ = rel_entry
        if highest_index > rel_idx:
            log.error(
                f"malformed type sequence: '{res[0]}' must come after '{types_lst}'"
            )
            sys.exit(errno.EINVAL)
        highest_index = rel_idx

        if res[0] in found_types:
            log.error(f"multiple types '{res[0]}' found in provided types")
            sys.exit(errno.EINVAL)
        found_types.append(res[0])

        types_lst.append((res[0], res[1]))

    return types_lst


def _parse_components(components: list[str]) -> dict[str, str]:
    comps: dict[str, str] = {}

    for c in components:
        m = re.match(r"^([\w_-]+)@([\d\w_.-]+)$", c)
        if not m:
            log.error(f"malformed component name/version pair '{c}'")
            sys.exit(errno.EINVAL)
        comps[m.group(1)] = m.group(2)

    return comps


def _parse_component_overrides(overrides: list[str]) -> dict[str, str]:
    override_map: dict[str, str] = {}

    regex = re.compile(r"^([\w_-]+)=([\d\w_.:/-]+)$")
    for override in overrides:
        m = re.match(regex, override)
        if not m:
            log.error(f"malformed component override '{override}'")
            sys.exit(errno.EINVAL)
        override_map[m.group(1)] = m.group(2)

    return override_map


def _get_build_type(types_lst: list[tuple[str, int]]) -> BuildType:
    if len(types_lst) == 0:
        return BuildType.RELEASE

    what: BuildType | None = None
    for t, _ in types_lst:
        assert t in release_types
        if what is not None:
            what = BuildType.TESTING
            break

        if release_types[t][0] <= Prios.RC.value:
            what = BuildType.RELEASE
        else:
            what = BuildType.TESTING
            break

    assert what is not None
    return what


_create_help_msg = f"""Creates a new version descriptor file.

Requires a VERSION to be provided, which this descriptor describes.
VERSION must include the "CES" prefix if a CES version is intended. Otherwise,
it can be free-form as long as it starts with a 'v' (such as, 'v18.2.4').

Requires at least one '--type TYPE=N' pair, specifying which type of release
the version refers to.

Requires all components to be passed as '--component NAME@VERSION', individually.

Available components: {", ".join(component_repos.keys())}.
"""


@main.command("create", help=_create_help_msg)
@click.argument("version", type=str)
@click.option(
    "-t",
    "--type",
    "build_types",
    type=str,
    multiple=True,
    help="Type of build, and its iteration",
    required=False,
    metavar="TYPE=N",
)
@click.option(
    "-c",
    "--component",
    "components",
    type=str,
    multiple=True,
    required=True,
    metavar="NAME@VERSION",
    help="Component's versions (e.g., 'ceph@ces-v24.11.0-ga.1')",
)
@click.option(
    "-o",
    "--override-component",
    "component_overrides",
    type=str,
    multiple=True,
    help="Override component's locations",
    required=False,
    metavar="COMPONENT=URL",
)
@click.option(
    "--distro",
    type=str,
    help="Distribution to use for this release",
    required=False,
    default="rockylinux:9",
    metavar="NAME",
)
@click.option(
    "--el-version",
    type=int,
    help="Distribution EL version",
    required=False,
    default=9,
    metavar="VERSION",
)
@click.option(
    "--registry",
    type=str,
    help="Registry for this release's image",
    required=False,
    default="harbor.clyso.com",
    metavar="URL",
)
@click.option(
    "--image-name",
    type=str,
    help="Name for this release's image",
    required=False,
    default="ces/ceph/ceph",
    metavar="NAME",
)
@click.option(
    "--image-tag",
    type=str,
    help="Tag for this release's image",
    required=False,
    metavar="TAG",
)
def build_create(
    version: str,
    build_types: list[str],
    components: list[str],
    component_overrides: list[str],
    distro: str,
    el_version: int,
    registry: str,
    image_name: str,
    image_tag: str | None,
):
    if not _validate_version(version):
        log.error(f"malformed version '{version}'")
        sys.exit(errno.EINVAL)

    types_lst = _parse_types(build_types)
    build_type_dir_name = (
        "testing" if _get_build_type(types_lst) == BuildType.TESTING else "releases"
    )

    components_map = _parse_components(components)
    if len(components_map) == 0:
        log.error("missing valid components")
        sys.exit(errno.EINVAL)

    for c in components_map.keys():
        if c not in component_repos:
            log.error(f"unknown component '{c}' specified")
            sys.exit(errno.ENOENT)

    component_overrides_map = _parse_component_overrides(component_overrides)
    for c in component_overrides_map:
        if c not in components_map:
            log.error(f"missing component '{c}' for override")
            sys.exit(errno.ENOENT)

    version_types = "-".join([f"{t}.{n}" for t, n in types_lst])
    version_str = f"{version}" + (f"-{version_types}" if version_types else "")
    version_types_title = " ".join(
        [f"{release_types[t][1]} #{n}" for t, n in types_lst]
    )
    version_title = f"Release {version}" + (
        f", {version_types_title}" if version_types_title else ""
    )

    print(f"version types: {version_types}")
    print(f"version str: {version_str}")
    print(f"version types title: {version_types_title}")
    print(f"version title: {version_title}")

    user_name, user_email = get_git_user()

    repo_path = get_git_repo_root()
    version_path = (
        repo_path.joinpath("versions")
        .joinpath(build_type_dir_name)
        .joinpath(f"{version_str}.json")
    )
    if version_path.exists():
        log.error(f"version for {version_str} already exists")
        sys.exit(errno.EEXIST)

    version_path.parent.mkdir(parents=True, exist_ok=True)

    component_res: list[VersionComponent] = []
    for comp_name, comp_version in components_map.items():
        comp_repo = component_repos[comp_name]
        if comp_name in component_overrides_map:
            comp_repo = component_overrides_map[comp_name]

        component_res.append(
            VersionComponent(name=comp_name, repo=comp_repo, version=comp_version)
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
    desc_json = desc.model_dump_json(indent=2)
    print(desc_json)

    try:
        desc.write(version_path)
    except Exception as e:
        log.error(f"unable to write descriptor at '{version_path}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    log.info(f"-> written to {version_path}")

    # check if image descriptor for this version exists
    try:
        _ = get_image_desc(version_str)
    except NoSuchVersionError:
        log.warning(f"image descriptor for version '{version_str}' missing")
    except CESError as e:
        log.error(f"error obtaining image descriptor for '{version_str}': {e}")


if __name__ == "__main__":
    main()
