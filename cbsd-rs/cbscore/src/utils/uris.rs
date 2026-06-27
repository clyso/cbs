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

//! Loose URI prefix matching for secret selection (design 004). Source:
//! `cbscore/utils/uris.py`. This is the machinery `find_best_secret_candidate`
//! (`utils::secrets::utils`) uses to pick the configured secret whose key is the
//! closest-matching prefix of a target git/registry URL.
//!
//! **Divergence from Python (documented):** `uris.py:75` interpolates the raw
//! pattern path into a regex, which would mishandle regex metacharacters in a
//! path. The port does the prefix check by **path segment** (split on `/`),
//! which is equivalent for real inputs and avoids that footgun. The Python
//! "unexpected empty remainder" `URIError` (`uris.py:81-87`) is therefore
//! structurally impossible here — equal segment lists are reported as
//! [`UriMatch::Full`] before any remainder is computed — so [`matches_uri`] is
//! infallible. The golden test mirrors `uris.py`'s own case table.

use std::sync::LazyLock;

use regex::Regex;

/// How a pattern matches a URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UriMatch {
    /// Protocol or host disagree, or the pattern path is not a segment-prefix.
    No,
    /// Host (and protocol, when both name one) agree and the paths are equal.
    Full,
    /// The pattern path is a strict segment-prefix of the URI's; `remainder` is
    /// the rest of the URI path, used to rank candidates (shorter wins).
    Partial { remainder: String },
}

/// A loose git-ish URI: an optional scheme, a host, and a `/segment` path.
/// Mirrors `uris.py`'s `uri_re` — deliberately simpler than the credential-aware
/// `GIT_URL_PATTERN` (no user/port), because it matches secret *keys* like
/// `github.com` or `github.com/ceph`.
static URI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        ^
        (?:(?P<protocol>git|https?|ssh)://)?
        (?P<host>[\w.\-]+)
        (?P<path>(?:/[\w.\-]+)*)?
        /?
        $
        ",
    )
    .expect("uri grammar regex is valid")
});

struct UriParts {
    protocol: Option<String>,
    host: String,
    path: String,
}

/// Parse a pattern or URI into `(protocol?, host, path)`, dropping a trailing
/// `.git` first (as `uris.py:47-48` does for both sides). `None` if it does not
/// look like a URI at all.
fn parse_uri(s: &str) -> Option<UriParts> {
    let s = s.strip_suffix(".git").unwrap_or(s);
    let caps = URI_RE.captures(s)?;
    Some(UriParts {
        protocol: caps.name("protocol").map(|m| m.as_str().to_string()),
        host: caps.name("host")?.as_str().to_string(),
        path: caps
            .name("path")
            .map(|m| m.as_str().to_string())
            .unwrap_or_default(),
    })
}

/// Non-empty path segments (`/a/b/` → `["a", "b"]`).
fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// Match `pattern` against `uri`. Protocols must agree only when both name one;
/// hosts must always agree; the pattern path must be a segment-prefix of the
/// URI path (`uris.py:27-89`).
pub fn matches_uri(pattern: &str, uri: &str) -> UriMatch {
    let (Some(p), Some(u)) = (parse_uri(pattern), parse_uri(uri)) else {
        return UriMatch::No;
    };
    if let (Some(pp), Some(up)) = (&p.protocol, &u.protocol)
        && pp != up
    {
        return UriMatch::No;
    }
    if p.host != u.host {
        return UriMatch::No;
    }

    let pseg = segments(&p.path);
    let useg = segments(&u.path);
    if pseg == useg {
        return UriMatch::Full;
    }
    if pseg.len() < useg.len() && useg[..pseg.len()] == pseg[..] {
        return UriMatch::Partial {
            remainder: useg[pseg.len()..].join("/"),
        };
    }
    UriMatch::No
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(remainder: &str) -> UriMatch {
        UriMatch::Partial {
            remainder: remainder.to_string(),
        }
    }

    #[test]
    fn matches_uri_golden_cases() {
        // Ported from cbscore/utils/uris.py:100-112.
        let cases: &[(&str, &str, UriMatch)] = &[
            ("https://github.com", "https://github.com", UriMatch::Full),
            ("github.com", "https://github.com", UriMatch::Full),
            ("github.com", "https://github.com/ceph", partial("ceph")),
            (
                "github.com",
                "https://github.com/ceph/ceph",
                partial("ceph/ceph"),
            ),
            ("foobar.com", "https://github.com/ceph/ceph", UriMatch::No),
            ("harbor.foo.tld", "https://harbor.foo.tld", UriMatch::Full),
            (
                "harbor.foo.tld/projects",
                "https://harbor.foo.tld",
                UriMatch::No,
            ),
            (
                "harbor.foo.tld",
                "https://harbor.foo.tld/projects",
                partial("projects"),
            ),
        ];
        for (pattern, uri, expected) in cases {
            assert_eq!(
                &matches_uri(pattern, uri),
                expected,
                "matching '{pattern}' against '{uri}'"
            );
        }
    }

    #[test]
    fn dot_git_suffix_is_ignored_on_both_sides() {
        assert_eq!(
            matches_uri("github.com/ceph/ceph.git", "https://github.com/ceph/ceph"),
            UriMatch::Full
        );
        assert_eq!(
            matches_uri("github.com", "https://github.com/ceph/ceph.git"),
            partial("ceph/ceph")
        );
    }

    #[test]
    fn protocol_mismatch_only_when_both_present() {
        // Both specify a (different) protocol → no match.
        assert_eq!(
            matches_uri("git://github.com", "https://github.com"),
            UriMatch::No
        );
        // Pattern omits the protocol → protocol is not compared.
        assert_eq!(
            matches_uri("github.com", "ssh://github.com"),
            UriMatch::Full
        );
    }

    #[test]
    fn a_longer_pattern_path_never_matches_a_shorter_uri() {
        assert_eq!(
            matches_uri("github.com/ceph/ceph", "https://github.com/ceph"),
            UriMatch::No
        );
    }
}
