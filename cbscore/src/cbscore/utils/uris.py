# CES library - URIs utilities
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

# partially adapted from a github copilot generated pattern to validate git urls
import re

from cbscore.errors import CESError
from cbscore.utils import logger as parent_logger

logger = parent_logger.getChild("uris")


class URIError(CESError):
    pass


def matches_uri(pattern: str, uri: str) -> tuple[bool, bool, str | None]:
    """
    Match a given pattern against the provided URI.

    Returns a tuple of bools, indicating whether the pattern is a match, and whether
    it is a full match on the path. Additionally, if it's a partial match, return the
    remainder path.
    """
    uri_re = re.compile(
        r"""
        ^
        (?:(?P<protocol>git|https?|ssh)://)?
        (?P<host>[\w\.\-]+)
        (?P<path>(?:/[\w\.\-]+)*)?/?
        $
        """,
        re.VERBOSE,
    )

    # drop '.git' suffix from both pattern and url for matching purposes, if any.
    pattern = re.sub(r"\.git$", "", pattern)
    uri = re.sub(r"\.git$", "", uri)

    pattern_m = uri_re.match(pattern)
    uri_m = uri_re.match(uri)
    if not pattern_m or not uri_m:
        return (False, False, None)

    if (
        pattern_m.group("protocol")
        and uri_m.group("protocol")
        and pattern_m.group("protocol") != uri_m.group("protocol")
    ):
        return (False, False, None)

    if pattern_m.group("host") != uri_m.group("host"):
        return (False, False, None)

    pattern_path = pattern_m.group("path") or ""
    url_path = uri_m.group("path") or ""
    if not pattern_path and not url_path:
        return (True, True, None)

    # Ensure pattern path is a prefix of target path, and matches full segments
    if pattern_path == url_path:
        return (True, True, None)

    adjusted_pattern_path = pattern_path.rstrip("/")
    path_pattern_re = re.compile(rf"^{adjusted_pattern_path}(?:/|$)(?P<remainder>.*)$")
    remainder_m = path_pattern_re.match(url_path)
    if not remainder_m:
        # did not match at all, must not match.
        return (False, False, None)

    if not remainder_m.group("remainder"):
        msg = (
            f"unexpected empty remainder when matching git url '{uri}' "
            + f"against pattern '{pattern}'"
        )
        logger.error(msg)
        raise URIError(msg)

    return (True, False, remainder_m.group("remainder"))


#
# kludge to test uri matching.
#
_check_mark = "\u2714"  # ✔
_error_mark = "\u274c"  # ❌


if __name__ == "__main__":
    _test_cases = [
        ("https://github.com", "https://github.com", (True, True, None)),
        ("github.com", "https://github.com", (True, True, None)),
        ("github.com", "https://github.com/ceph", (True, False, "ceph")),
        ("github.com", "https://github.com/ceph/ceph", (True, False, "ceph/ceph")),
        ("foobar.com", "https://github.com/ceph/ceph", (False, False, None)),
        ("harbor.foo.tld", "https://harbor.foo.tld", (True, True, None)),
        ("harbor.foo.tld/projects", "https://harbor.foo.tld", (False, False, None)),
        (
            "harbor.foo.tld",
            "https://harbor.foo.tld/projects",
            (True, False, "projects"),
        ),
    ]

    for case in _test_cases:
        pattern, uri, expected = case
        try:
            result = matches_uri(pattern, uri)
        except URIError as e:
            print(
                f"{_error_mark} URIError for pattern '{pattern}' and uri '{uri}': {e}"
            )
            continue

        if result != expected:
            print(
                f"{_error_mark} Failed test case for pattern '{pattern}' and "
                + f"uri '{uri}': expected {expected}, got {result}"
            )
            continue

        print(f"{_check_mark} Passed test case for pattern '{pattern}' and uri '{uri}'")
