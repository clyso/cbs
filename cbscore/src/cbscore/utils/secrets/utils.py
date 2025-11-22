# CES library - secrets utilities - common utilities
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

from cbscore.utils.secrets import logger as parent_logger
from cbscore.utils.uris import URIError, matches_uri

logger = parent_logger.getChild("utils")


def find_best_secret_candidate(secrets: list[str], uri: str) -> str | None:
    """Find the best candidate secret from a list that matches a uri most closely."""
    best_candidate: tuple[str, str | None] | None = None
    for target in secrets:
        try:
            matches, full_match, remainder = matches_uri(target, uri)
        except URIError as e:
            logger.error(
                f"unexpected error matching uri '{uri}' against '{target}': {e}"
            )
            continue

        if not matches:
            continue
        if full_match:
            return target

        if not best_candidate or (not best_candidate[1] and remainder):
            best_candidate = (target, remainder)
            continue

        if (
            best_candidate[1]
            and remainder
            and best_candidate[1].count("/") > remainder.count("/")
        ):
            best_candidate = (target, remainder)

    return best_candidate[0] if best_candidate else None


#
# kludge to test finding the best secret candidate from a list of secrets.
#
_check_mark = "\u2714"  # ✔
_error_mark = "\u274c"  # ❌


if __name__ == "__main__":
    _test_cases: list[tuple[list[str], str, str | None]] = [
        ([], "foo.bar.tld", None),
        (["foo.bar.tld"], "foo.bar.baz", None),
        (["foo.bar.tld", "foo.baz.tld"], "foo.bar.baz", None),
        (["foo.bar.tld", "foo.baz.tld"], "foo.bar.tld", "foo.bar.tld"),
        (["foo.bar.tld", "foo.baz.tld"], "foo.bar.tld/foobar", "foo.bar.tld"),
        (["foo.bar.tld/foobar", "foo.baz.tld"], "foo.bar.tld", None),
        (
            ["foo.bar.tld/foobar", "foo.baz.tld"],
            "foo.bar.tld/foobar",
            "foo.bar.tld/foobar",
        ),
        (
            ["foo.bar.tld/foo", "foo.bar.tld/foo/bar"],
            "foo.bar.tld/foo",
            "foo.bar.tld/foo",
        ),
        (
            ["foo.bar.tld/foo", "foo.bar.tld/foo/bar", "foo.bar.tld/baz"],
            "foo.bar.tld/foo/bar",
            "foo.bar.tld/foo/bar",
        ),
        (
            ["foo.bar.tld/foo", "foo.bar.tld/bar"],
            "foo.bar.tld/foo/bar",
            "foo.bar.tld/foo",
        ),
    ]

    for case in _test_cases:
        secrets, uri, expected = case
        result = find_best_secret_candidate(secrets, uri)

        if result != expected:
            print(
                f"{_error_mark} Failed test case for secrets '{secrets}' and "
                + f"uri '{uri}': expected {expected}, got {result}"
            )
            continue

        print(f"{_check_mark} Passed test case for secrets '{secrets}' and uri '{uri}'")
