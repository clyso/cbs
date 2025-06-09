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


from crtlib.models.patch import Patch
from rich.tree import Tree


def print_patch_tree(what: str, lst: list[Patch]) -> None:
    tree = Tree(f"\u29bf {what}:")
    for patch in lst:
        _ = tree.add(f"{patch.title} ({patch.sha})")
