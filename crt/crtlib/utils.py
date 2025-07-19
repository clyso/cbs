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
