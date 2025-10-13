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


import re
from pathlib import Path
from typing import cast

from cbscore.versions.utils import parse_version
from rich.tree import Tree

from crtlib.errors.stages import MalformedStageTagError
from crtlib.models.patch import Patch


def print_patch_tree(what: str, lst: list[Patch]) -> None:
    tree = Tree(f"\u29bf {what}:")
    for patch in lst:
        _ = tree.add(f"{patch.title} ({patch.sha})")


def get_tags(tags_lst: list[str] | None) -> list[tuple[str, str]]:
    if not tags_lst:
        return []

    tags: list[tuple[str, str]] = []
    tag_re = re.compile(r"^(?P<tag>\w+)=(?P<n>\w+)")
    for t in tags_lst:
        if m := tag_re.match(t):
            tags.append((cast(str, m.group("tag")), cast(str, m.group("n"))))
        else:
            raise MalformedStageTagError(msg=f"tag '{t}'")

    return tags


def split_version_into_paths(
    version: str, with_patch_and_suffix: bool = True
) -> list[Path]:
    def _parse_version_hierarchy() -> list[str]:
        prefix, major, minor, patch, suffix = parse_version(version)

        levels: list[str] = []
        base = f"v{major}"
        levels.append(base)
        if minor:
            base = f"{base}.{minor}"
            levels.append(base)
        if patch:
            base = f"{base}.{patch}"
            if with_patch_and_suffix and suffix:
                base = f"{base}.{patch}-{suffix}"
            levels.append(base)

        if prefix:
            levels = [f"{prefix}-{lvl}" for lvl in levels]
        if suffix and not with_patch_and_suffix:
            last = next(reversed(levels))
            levels.append(f"{last}-{suffix}")

        return levels

    paths_lst: list[Path] = []
    for p in _parse_version_hierarchy():
        last_elem_path = next(reversed(paths_lst)) if paths_lst else Path()
        part_path = last_elem_path.joinpath(p)
        paths_lst.append(part_path)

    return paths_lst
