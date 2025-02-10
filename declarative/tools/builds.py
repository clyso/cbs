#!/usr/bin/env python3

# Handles CES declarative builds
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
import json
import logging
import re
import sys
from typing import cast

import click
from ceslib.errors import CESError
from ceslib.images.desc import get_version_desc
from ceslib.images.errors import NoSuchVersionError
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
    return re.match(r"^\d{2}\.\d{2}\.\d+$", v) is not None


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


_create_help_msg = f"""Creates a new build descriptor file.

Requires a VERSION to be provided, which this descriptor describes.

Requires at least one '--type TYPE=N' pair, specifying which type of release
the build refers to.

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
    required=True,
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
def build_create(
    version: str,
    build_types: list[str],
    components: list[str],
    component_overrides: list[str],
):
    if len(build_types) == 0:
        log.error("no build type provided")
        sys.exit(errno.EINVAL)

    if not _validate_version(version):
        log.error(f"malformed version '{version}'")
        sys.exit(errno.EINVAL)

    types_lst = _parse_types(build_types)
    if len(types_lst) == 0:
        log.error("missing valid build type")
        sys.exit(errno.EINVAL)

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

    ces_version_types = "-".join([f"{t}.{n}" for t, n in types_lst])
    raw_version_str = f"{version}-{ces_version_types}"
    ces_version = f"ces-v{raw_version_str}"
    version_types_title = " ".join(
        [f"{release_types[t][1]} #{n}" for t, n in types_lst]
    )
    ces_version_title = f"Release CES v{version}, {version_types_title}"

    user_name, user_email = get_git_user()

    repo_path = get_git_repo_root()
    build_path = (
        repo_path.joinpath("builds")
        .joinpath(build_type_dir_name)
        .joinpath(f"{ces_version}.json")
    )
    if build_path.exists():
        log.error(f"build for {ces_version} already exists")
        sys.exit(errno.EEXIST)

    build_path.parent.mkdir(parents=True, exist_ok=True)

    component_res: list[dict[str, str]] = []
    for comp_name, comp_version in components_map.items():
        comp_repo = component_repos[comp_name]
        if comp_name in component_overrides_map:
            comp_repo = component_overrides_map[comp_name]

        component_res.append(
            {"name": comp_name, "repo": comp_repo, "version": comp_version}
        )

    res_dict = {
        "version": ces_version,
        "title": ces_version_title,
        "signed-off-by": {
            "user": user_name,
            "email": user_email,
        },
        "components": component_res,
    }

    json_str = json.dumps(res_dict, indent=2)
    print(json_str)

    with build_path.open("w") as f:
        print(json_str, file=f)
        log.info(f"-> written to {build_path}")

    # check if image descriptor for this version exists
    try:
        _ = get_version_desc(raw_version_str)
    except NoSuchVersionError:
        log.warning(f"image descriptor for version '{raw_version_str}' missing")
    except CESError as e:
        log.error(f"error obtaining image descriptor for '{raw_version_str}': {e}")


if __name__ == "__main__":
    main()
