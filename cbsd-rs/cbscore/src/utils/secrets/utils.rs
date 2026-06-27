// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Secret-candidate selection (design 004). Source:
//! `cbscore/utils/secrets/utils.py`.

use crate::utils::uris::{UriMatch, matches_uri};

/// From `keys`, pick the secret key that matches `uri` most closely
/// (`utils.py:20-48`): a full match wins immediately; otherwise the candidate
/// whose unmatched **remainder** is shortest (fewest path segments) wins, with
/// the first such candidate winning ties. Returns `None` when nothing matches.
pub fn find_best_secret_candidate<'a, I>(keys: I, uri: &str) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best: Option<(&'a str, String)> = None;
    for key in keys {
        match matches_uri(key, uri) {
            UriMatch::No => continue,
            UriMatch::Full => return Some(key),
            UriMatch::Partial { remainder } => match &best {
                None => best = Some((key, remainder)),
                Some((_, best_remainder)) => {
                    // A shorter remainder means a longer prefix matched, i.e. a
                    // closer secret. Strict `>` keeps the first on a tie.
                    if best_remainder.matches('/').count() > remainder.matches('/').count() {
                        best = Some((key, remainder));
                    }
                }
            },
        }
    }
    best.map(|(key, _)| key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best<'a>(keys: &[&'a str], uri: &str) -> Option<&'a str> {
        find_best_secret_candidate(keys.iter().copied(), uri)
    }

    #[test]
    fn find_best_secret_candidate_golden_cases() {
        // Ported from cbscore/utils/secrets/utils.py:59-86.
        assert_eq!(best(&[], "foo.bar.tld"), None);
        assert_eq!(best(&["foo.bar.tld"], "foo.bar.baz"), None);
        assert_eq!(best(&["foo.bar.tld", "foo.baz.tld"], "foo.bar.baz"), None);
        assert_eq!(
            best(&["foo.bar.tld", "foo.baz.tld"], "foo.bar.tld"),
            Some("foo.bar.tld")
        );
        assert_eq!(
            best(&["foo.bar.tld", "foo.baz.tld"], "foo.bar.tld/foobar"),
            Some("foo.bar.tld")
        );
        assert_eq!(
            best(&["foo.bar.tld/foobar", "foo.baz.tld"], "foo.bar.tld"),
            None
        );
        assert_eq!(
            best(&["foo.bar.tld/foobar", "foo.baz.tld"], "foo.bar.tld/foobar"),
            Some("foo.bar.tld/foobar")
        );
        assert_eq!(
            best(
                &["foo.bar.tld/foo", "foo.bar.tld/foo/bar"],
                "foo.bar.tld/foo"
            ),
            Some("foo.bar.tld/foo")
        );
        assert_eq!(
            best(
                &["foo.bar.tld/foo", "foo.bar.tld/foo/bar", "foo.bar.tld/baz"],
                "foo.bar.tld/foo/bar"
            ),
            Some("foo.bar.tld/foo/bar")
        );
        // The closer (longer) prefix wins even when listed first vs. second.
        assert_eq!(
            best(
                &["foo.bar.tld/foo", "foo.bar.tld/bar"],
                "foo.bar.tld/foo/bar"
            ),
            Some("foo.bar.tld/foo")
        );
    }

    #[test]
    fn a_real_git_url_selects_the_host_prefix_secret() {
        // The secret key is a bare host; the URL is a full clone URL.
        assert_eq!(
            best(
                &["github.com", "gitlab.com"],
                "https://github.com/ceph/ceph"
            ),
            Some("github.com")
        );
        // A more specific org-scoped key beats the bare host.
        assert_eq!(
            best(
                &["github.com", "github.com/ceph"],
                "https://github.com/ceph/ceph"
            ),
            Some("github.com/ceph")
        );
    }
}
