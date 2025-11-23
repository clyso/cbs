# CES library - container utilities
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

from cbscore.versions.desc import VersionDescriptor


def get_container_image_base_uri(desc: VersionDescriptor | str) -> str:
    """Return the container's registry URI including path."""
    if isinstance(desc, VersionDescriptor):
        return f"{desc.image.registry}/{desc.image.name}"

    assert isinstance(desc, str)
    uri_m = re.match(
        r"""
        ^
        (?P<base>[^:@]+)
        (?:[:@].*)?
        $""",
        desc,
        re.VERBOSE,
    )
    if not uri_m:
        raise ValueError(f"malformed container image uri '{desc}'")

    return cast(str, uri_m.group("base"))


def get_container_canonical_uri(
    desc: VersionDescriptor, *, digest: str | None = None
) -> str:
    """Return the container image's canonical URI (minus transport)."""
    tag = desc.image.tag

    return get_container_image_base_uri(desc) + (f"@{digest}" if digest else f":{tag}")
