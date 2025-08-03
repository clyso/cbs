# crt - utils
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
import re
from typing import cast

from crtlib.errors.stages import MalformedStageTagError
from crtlib.models.patch import Patch
from rich.tree import Tree


def print_patch_tree(what: str, lst: list[Patch]) -> None:
    tree = Tree(f"\u29bf {what}:")
    for patch in lst:
        _ = tree.add(f"{patch.title} ({patch.sha})")


def get_tags(tags_lst: list[str] | None) -> list[tuple[str, int]]:
    if not tags_lst:
        return []

    tags: list[tuple[str, int]] = []
    tag_re = re.compile(r"^(?P<tag>\w+)=(?P<n>\d+)")
    for t in tags_lst:
        if m := tag_re.match(t):
            tags.append((cast(str, m.group("tag")), int(m.group("n"))))
        else:
            raise MalformedStageTagError(msg=f"tag '{t}'")

    return tags


def split_version_into_paths(version: str) -> list[Path]:
    def _parse_version_hierarchy() -> list[str]:
        v_re = re.compile(
            r"""
            ^(?P<prefix>.*?)(?:-)?  # optional prefix plus dash
            v
            (?P<major>\d{2})        # major version (required)
            (?:\.(?P<minor>\d{2}))? # minor version (optional)
            (?:\.(?P<patch>\d+))?   # patch version (optional)
            (?:-(?P<suffix>.+))?    # dash with suffix (optional)
            $
            """,
            re.VERBOSE,
        )
        m = v_re.match(version)
        if not m:
            raise ValueError(f"invalid version '{version}'")  # noqa: TRY003

        prefix = cast(str | None, m.group("prefix"))
        major = cast(str, m.group("major"))
        minor = cast(str | None, m.group("minor"))
        patch = cast(str | None, m.group("patch"))
        suffix = cast(str | None, m.group("suffix"))

        levels: list[str] = []
        base = f"v{major}"
        levels.append(base)
        if minor:
            base = f"{base}.{minor}"
            levels.append(base)
        if patch:
            base = f"{base}.{patch}"
            levels.append(base)

        if prefix:
            levels = [f"{prefix}-{lvl}" for lvl in levels]
        if suffix:
            last = next(reversed(levels))
            levels.append(f"{last}-{suffix}")

        return levels

    paths_lst: list[Path] = []
    for p in _parse_version_hierarchy():
        last_elem_path = next(reversed(paths_lst)) if paths_lst else Path()
        part_path = last_elem_path.joinpath(p)
        paths_lst.append(part_path)

    return paths_lst
