# CES library - versions utils
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

from cbscore.errors import MalformedVersionError
from cbscore.versions import logger as parent_logger
from cbscore.versions.errors import VersionError

logger = parent_logger.getChild("utils")


class VersionType(enum.StrEnum):
    RELEASE = "release"
    DEV = "dev"
    TEST = "test"
    CI = "ci"


_release_types: dict[str, tuple[VersionType, str]] = {
    "release": (VersionType.RELEASE, "General Availability"),
    "dev": (VersionType.DEV, "Development"),
    "test": (VersionType.TEST, "Testing"),
    "ci": (VersionType.CI, "CI/CD"),
}


ParseVersionResult = tuple[str | None, str, str | None, str | None, str | None]


def parse_version(version: str) -> ParseVersionResult:
    v_re = re.compile(
        r"""
        ^
        (?:(?P<prefix>(\w+))-)?         # optional prefix
        v?                              # optional 'v'
        (?P<major>\d+)                  # mandatory major version
        (?:\.(?P<minor>\d+)             # optional minor version
            (?:\.(?P<patch>\d+)         # optional patch version
            (?:-(?P<suffix>[\w_.-]+))?  # optional suffix
            )?
        )?
        $
        """,
        re.VERBOSE,
    )
    m = v_re.match(version)
    if not m:
        raise MalformedVersionError(f"invalid version '{version}'")

    prefix = cast(str | None, m.group("prefix"))
    major = cast(str, m.group("major"))
    minor = cast(str | None, m.group("minor"))
    patch = cast(str | None, m.group("patch"))
    suffix = cast(str | None, m.group("suffix"))

    return (prefix, major, minor, patch, suffix)


def get_major_version(v: str) -> str:
    """Obtain the major version from the supplied version."""
    # Keep in mind 'parse_version()' is an agnostic version parser.
    # It doesn't understand how we consider major and minor versions
    # for CES or Ceph. So, a major version for CES/Ceph will be the
    # first two version components: major and minor.
    try:
        _, major, minor, _, _ = parse_version(v)
    except MalformedVersionError as e:
        raise e from None

    if not major or not minor:
        raise MalformedVersionError(v)

    return f"{major}.{minor}"


def get_minor_version(v: str) -> str | None:
    """Obtain the minor version from the supplied version."""
    # Keep in mind 'parse_version()' is an agnostic version parser.
    # It doesn't understand how we consider major and minor versions
    # for CES or Ceph. So, a minor version for CES/Ceph will be the three
    # version components: major, minor, and patch.
    try:
        _, major, minor, patch, _ = parse_version(v)
    except MalformedVersionError as e:
        raise e from None

    if not major or not minor or not patch:
        return None

    return f"{major}.{minor}.{patch}"


def normalize_version(v: str) -> str:
    try:
        prefix, major, minor, patch, suffix = parse_version(v)
    except MalformedVersionError as e:
        raise e from None

    if not major or not minor:
        raise MalformedVersionError(v)

    res = ""
    if prefix:
        res += f"{prefix}-"
    res += f"v{major}.{minor}"
    if patch:
        res += f".{patch}"
    if suffix:
        res += f"-{suffix}"
    return res


def get_version_type(type_name: str) -> VersionType:
    if v := _release_types.get(type_name.lower(), None):
        return v[0]
    msg = f"unknown version type '{type_name}'"
    logger.error(msg)
    raise VersionError(msg)


def get_version_type_desc(version_type: VersionType) -> str:
    if version_type.value not in _release_types:
        msg = f"unknown version type '{version_type.value}'"
        logger.error(msg)
        raise VersionError(msg)
    return _release_types[version_type.value][1]


def parse_component_refs(components: list[str]) -> dict[str, str]:
    """Parse a list of strings made of components in the format 'COMPONENT@REF'."""
    comps: dict[str, str] = {}

    for c in components:
        m = re.match(r"^([\w_-]+)@([\d\w_./-]+)$", c)
        if not m:
            msg = f"malformed component name/version pair '{c}'"
            logger.error(msg)
            raise VersionError(msg)
        comps[m.group(1)] = m.group(2)

    return comps


if __name__ == "__main__":
    overall_success = True

    version_tests: list[tuple[str, bool, ParseVersionResult | None]] = [
        # valid patterns
        ("ces-v99.99.1-asd-qwe", True, ("ces", "99", "99", "1", "asd-qwe")),
        ("ces-v99.99.1-asd", True, ("ces", "99", "99", "1", "asd")),
        ("ces-v99.99.1", True, ("ces", "99", "99", "1", None)),
        ("ces-v99.99", True, ("ces", "99", "99", None, None)),
        ("ces-v99", True, ("ces", "99", None, None, None)),
        ("ces-99.99.1-asd", True, ("ces", "99", "99", "1", "asd")),
        ("ces-99.99.1", True, ("ces", "99", "99", "1", None)),
        ("ces-99.99", True, ("ces", "99", "99", None, None)),
        ("ces-99", True, ("ces", "99", None, None, None)),
        ("v99.99.1-asd", True, (None, "99", "99", "1", "asd")),
        ("v99.99.1", True, (None, "99", "99", "1", None)),
        ("v99.99", True, (None, "99", "99", None, None)),
        ("v99", True, (None, "99", None, None, None)),
        ("99.99.1-asd", True, (None, "99", "99", "1", "asd")),
        ("99.99.1", True, (None, "99", "99", "1", None)),
        ("99.99", True, (None, "99", "99", None, None)),
        ("99", True, (None, "99", None, None, None)),
        # invalid patterns
        ("ces", False, None),
        ("ces-", False, None),
        ("ces-v", False, None),
        ("-99.99.1-asd", False, None),
        ("-99", False, None),
        ("-v99", False, None),
        ("ces-99.", False, None),
        ("ces-99.99.", False, None),
        ("ces-v99.99.1-", False, None),
        ("ces-v99.99.1.", False, None),
        ("ces-v99-asd", False, None),
        ("ces-v99.asd", False, None),
        ("ces-asd", False, None),
        ("99.asd", False, None),
        ("99-asd", False, None),
        ("ces-.99.99.1-asd", False, None),
    ]

    print("running version tests...")
    for test in version_tests:
        ver, is_valid, expected = test
        success = False
        try:
            res = parse_version(ver)
            if not is_valid:
                print(f"ERROR: '{ver}' should be invalid!")
            else:
                if res != expected:
                    print(f"ERROR: '{ver}' parsed as {res}, not as {expected}!")
                else:
                    success = True
        except MalformedVersionError:
            if is_valid:
                print(f"ERROR: '{ver}' should be valid!")
            else:
                success = True
        print(f"test '{ver}', success = {success}")
        if not success:
            overall_success = False

    normalize_tests: list[tuple[str, bool, str | None]] = [
        ("ces-v99.99.1-asd", True, "ces-v99.99.1-asd"),
        ("ces-v99.99.1", True, "ces-v99.99.1"),
        ("ces-v99.99", True, "ces-v99.99"),
        ("ces-v99", False, None),
        ("ces-99.99.1-asd", True, "ces-v99.99.1-asd"),
        ("ces-99.99.1", True, "ces-v99.99.1"),
        ("ces-99.99", True, "ces-v99.99"),
        ("ces-99", False, None),
        ("v99.99.1-asd", True, "v99.99.1-asd"),
        ("v99.99.1", True, "v99.99.1"),
        ("v99.99", True, "v99.99"),
        ("v99", False, None),
        ("99.99.1-asd", True, "v99.99.1-asd"),
        ("99.99.1", True, "v99.99.1"),
        ("99.99", True, "v99.99"),
        ("99", False, None),
        ("ces-v", False, None),
        ("ces-", False, None),
        ("ces", False, None),
    ]

    print("\nrunning normalize tests...")
    for test in normalize_tests:
        ver, is_valid, expected = test
        success = False
        try:
            res = normalize_version(ver)
            if not is_valid:
                print(f"ERROR: '{ver}' should be invalid!")
            else:
                if res != expected:
                    print(f"ERROR: '{ver}' normalized as {res}, not as {expected}!")
                else:
                    success = True
        except MalformedVersionError:
            if is_valid:
                print(f"ERROR: '{ver}' should be valid!")
            else:
                success = True

        print(f"test '{ver}', success = {success}")
        if not success:
            overall_success = False

    print(f"\noverall success = {overall_success}")
