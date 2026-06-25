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

//! Version-string grammar and parse helpers (design 006). Source:
//! `cbscore/versions/utils.py`. `parse_version` is an *agnostic* parser — it
//! does not encode CES's "major = first two components" convention; the
//! `get_major_version`/`get_minor_version` helpers do.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::types::Error;

/// The version grammar (`utils.py:44-59`):
/// `[<prefix>-] [v] <major> [.<minor> [.<patch> [-<suffix>]]]`.
static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        ^
        (?:(?P<prefix>\w+)-)?            # optional prefix
        v?                              # optional 'v'
        (?P<major>\d+)                  # mandatory major
        (?:\.(?P<minor>\d+)             # optional minor
            (?:\.(?P<patch>\d+)         # optional patch
                (?:-(?P<suffix>[\w_.-]+))?  # optional suffix
            )?
        )?
        $
        ",
    )
    .expect("version grammar regex is valid")
});

/// `COMPONENT@REF` grammar (`utils.py:148`).
static COMPONENT_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([\w_-]+)@([\d\w_./-]+)$").expect("component-ref regex is valid")
});

/// The parsed pieces of a version string (Python's 5-tuple). `major` is always
/// present; the rest are optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    pub prefix: Option<String>,
    pub major: String,
    pub minor: Option<String>,
    pub patch: Option<String>,
    pub suffix: Option<String>,
}

/// Parse a version string into its pieces; [`Error::MalformedVersion`] on no
/// match.
///
/// The match is anchored to end-of-text, so a trailing newline is *not*
/// accepted (Python's `$` matches before a final `\n`). Call sites pass CLI
/// args or a stored `desc.version`, never raw command output, so this is a
/// deliberate, benign divergence.
pub fn parse_version(version: &str) -> Result<ParsedVersion, Error> {
    let caps = VERSION_RE
        .captures(version)
        .ok_or_else(|| Error::MalformedVersion(format!("invalid version '{version}'")))?;
    let group = |name: &str| caps.name(name).map(|m| m.as_str().to_string());
    Ok(ParsedVersion {
        prefix: group("prefix"),
        major: caps
            .name("major")
            .expect("major is mandatory in a matched version")
            .as_str()
            .to_string(),
        minor: group("minor"),
        patch: group("patch"),
        suffix: group("suffix"),
    })
}

/// The CES "major" version — the first two components, `<major>.<minor>`. CES
/// treats the first two components as the "major" (not the first alone).
/// Requires both; [`Error::MalformedVersion`] otherwise.
pub fn get_major_version(v: &str) -> Result<String, Error> {
    let p = parse_version(v)?;
    match p.minor {
        Some(minor) => Ok(format!("{}.{}", p.major, minor)),
        None => Err(Error::MalformedVersion(v.to_string())),
    }
}

/// The CES "minor" version — `<major>.<minor>.<patch>` when patch is present,
/// else `None`. [`Error::MalformedVersion`] on a malformed string.
pub fn get_minor_version(v: &str) -> Result<Option<String>, Error> {
    let p = parse_version(v)?;
    match (p.minor, p.patch) {
        (Some(minor), Some(patch)) => Ok(Some(format!("{}.{}.{}", p.major, minor, patch))),
        _ => Ok(None),
    }
}

/// Re-emit a normalised `[<prefix>-]v<major>.<minor>[.<patch>][-<suffix>]`.
/// Requires major and minor.
pub fn normalize_version(v: &str) -> Result<String, Error> {
    let p = parse_version(v)?;
    let Some(minor) = p.minor else {
        return Err(Error::MalformedVersion(v.to_string()));
    };
    let mut res = String::new();
    if let Some(prefix) = p.prefix {
        res.push_str(&prefix);
        res.push('-');
    }
    res.push('v');
    res.push_str(&p.major);
    res.push('.');
    res.push_str(&minor);
    if let Some(patch) = p.patch {
        res.push('.');
        res.push_str(&patch);
    }
    if let Some(suffix) = p.suffix {
        res.push('-');
        res.push_str(&suffix);
    }
    Ok(res)
}

/// Parse `COMPONENT@REF` entries into a name→ref map. A later entry for the same
/// component overrides an earlier one (later-wins, as Python's `dict`); the map
/// iterates in sorted key order, not insertion order — key order is not
/// significant (design 002). [`Error::VersionError`] on a malformed entry.
pub fn parse_component_refs(components: &[String]) -> Result<BTreeMap<String, String>, Error> {
    let mut comps = BTreeMap::new();
    for c in components {
        let caps = COMPONENT_REF_RE.captures(c).ok_or_else(|| {
            Error::VersionError(format!("malformed component name/version pair '{c}'"))
        })?;
        comps.insert(caps[1].to_string(), caps[2].to_string());
    }
    Ok(comps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pv(
        prefix: Option<&str>,
        major: &str,
        minor: Option<&str>,
        patch: Option<&str>,
        suffix: Option<&str>,
    ) -> ParsedVersion {
        ParsedVersion {
            prefix: prefix.map(String::from),
            major: major.to_string(),
            minor: minor.map(String::from),
            patch: patch.map(String::from),
            suffix: suffix.map(String::from),
        }
    }

    #[test]
    fn parse_version_golden_cases() {
        // Ported verbatim from cbscore/versions/utils.py:161-197.
        let valid: &[(&str, ParsedVersion)] = &[
            (
                "ces-v99.99.1-asd-qwe",
                pv(Some("ces"), "99", Some("99"), Some("1"), Some("asd-qwe")),
            ),
            (
                "ces-v99.99.1-asd",
                pv(Some("ces"), "99", Some("99"), Some("1"), Some("asd")),
            ),
            (
                "ces-v99.99.1",
                pv(Some("ces"), "99", Some("99"), Some("1"), None),
            ),
            ("ces-v99.99", pv(Some("ces"), "99", Some("99"), None, None)),
            ("ces-v99", pv(Some("ces"), "99", None, None, None)),
            (
                "ces-99.99.1-asd",
                pv(Some("ces"), "99", Some("99"), Some("1"), Some("asd")),
            ),
            (
                "ces-99.99.1",
                pv(Some("ces"), "99", Some("99"), Some("1"), None),
            ),
            ("ces-99.99", pv(Some("ces"), "99", Some("99"), None, None)),
            ("ces-99", pv(Some("ces"), "99", None, None, None)),
            (
                "v99.99.1-asd",
                pv(None, "99", Some("99"), Some("1"), Some("asd")),
            ),
            ("v99.99.1", pv(None, "99", Some("99"), Some("1"), None)),
            ("v99.99", pv(None, "99", Some("99"), None, None)),
            ("v99", pv(None, "99", None, None, None)),
            (
                "99.99.1-asd",
                pv(None, "99", Some("99"), Some("1"), Some("asd")),
            ),
            ("99.99.1", pv(None, "99", Some("99"), Some("1"), None)),
            ("99.99", pv(None, "99", Some("99"), None, None)),
            ("99", pv(None, "99", None, None, None)),
        ];
        for (input, expected) in valid {
            assert_eq!(
                &parse_version(input).unwrap(),
                expected,
                "parsing '{input}'"
            );
        }

        let invalid = [
            "ces",
            "ces-",
            "ces-v",
            "-99.99.1-asd",
            "-99",
            "-v99",
            "ces-99.",
            "ces-99.99.",
            "ces-v99.99.1-",
            "ces-v99.99.1.",
            "ces-v99-asd",
            "ces-v99.asd",
            "ces-asd",
            "99.asd",
            "99-asd",
            "ces-.99.99.1-asd",
        ];
        for input in invalid {
            assert!(parse_version(input).is_err(), "'{input}' should be invalid");
        }
    }

    #[test]
    fn normalize_version_golden_cases() {
        // Ported verbatim from cbscore/versions/utils.py:221-241.
        let valid: &[(&str, &str)] = &[
            ("ces-v99.99.1-asd", "ces-v99.99.1-asd"),
            ("ces-v99.99.1", "ces-v99.99.1"),
            ("ces-v99.99", "ces-v99.99"),
            ("ces-99.99.1-asd", "ces-v99.99.1-asd"),
            ("ces-99.99.1", "ces-v99.99.1"),
            ("ces-99.99", "ces-v99.99"),
            ("v99.99.1-asd", "v99.99.1-asd"),
            ("v99.99.1", "v99.99.1"),
            ("v99.99", "v99.99"),
            ("99.99.1-asd", "v99.99.1-asd"),
            ("99.99.1", "v99.99.1"),
            ("99.99", "v99.99"),
        ];
        for (input, expected) in valid {
            assert_eq!(
                normalize_version(input).unwrap(),
                *expected,
                "normalizing '{input}'"
            );
        }

        let invalid = ["ces-v99", "ces-99", "v99", "99", "ces-v", "ces-", "ces"];
        for input in invalid {
            assert!(
                normalize_version(input).is_err(),
                "'{input}' should be invalid"
            );
        }
    }

    #[test]
    fn major_minor_helpers() {
        assert_eq!(get_major_version("ces-v20.2.1").unwrap(), "20.2");
        assert!(get_major_version("99").is_err()); // no minor
        assert_eq!(
            get_minor_version("20.2.1").unwrap(),
            Some("20.2.1".to_string())
        );
        assert_eq!(get_minor_version("20.2").unwrap(), None); // no patch
        assert!(get_minor_version("foobar").is_err());
    }

    #[test]
    fn component_refs_parse_dedupe_and_reject() {
        let got = parse_component_refs(&[
            "ceph@v20.2.1".to_string(),
            "dashboard@main".to_string(),
            "ceph@v20.2.2".to_string(), // later wins
        ])
        .unwrap();
        assert_eq!(got.get("ceph").map(String::as_str), Some("v20.2.2"));
        assert_eq!(got.get("dashboard").map(String::as_str), Some("main"));

        assert!(parse_component_refs(&["no-at-sign".to_string()]).is_err());
    }
}
