// CRT core — operator-authored patch annotations and version matching.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Operator-authored, mutable patch metadata (design seq-003 §3): a record
//! kept **separate** from the git-derived `PatchMeta`, recording which ceph
//! versions a patch applies to, free-form tags, a description, and an open
//! attribute bag. This module owns the pure types, the version parser (§5/§7),
//! and the applicability matching (§7); all IO lives in `crt-store` and the
//! flag→transition logic lives in `crt`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::CrtCoreError;

/// Current [`PatchAnnotations`] schema version. Bumped when a structural
/// change (e.g. graduating an `attributes` key to a typed field) makes an old
/// record a detectable migration rather than a silent misparse (design §3).
pub const ANNOTATIONS_SCHEMA_VERSION: u32 = 1;

/// Operator-authored metadata for one patch blob, stored at
/// `patches/annotations/sha256/<blob_hash>.json` (design §3). Mutable and
/// merged across re-imports — never overwritten like `PatchMeta`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchAnnotations {
    /// Schema version; see [`ANNOTATIONS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Which ceph versions this patch applies to. `None` = not yet assessed
    /// (never assumed generic; excluded from the version filter, design §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to: Option<Applicability>,
    /// Free-form categorization (e.g. `rgw`, `clyso/tunables`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    /// Hard-typed triage note ("what does this patch do?"), distinct from the
    /// upstream subject/body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Open key→value bag for facets without a typed field yet (e.g.
    /// `retire-when`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

impl Default for PatchAnnotations {
    /// An empty record at the current schema version: unassessed, no tags, no
    /// description, no attributes. The base for the import/`annotate` merge.
    fn default() -> Self {
        Self {
            schema_version: ANNOTATIONS_SCHEMA_VERSION,
            applies_to: None,
            tags: BTreeSet::new(),
            description: None,
            attributes: BTreeMap::new(),
        }
    }
}

impl PatchAnnotations {
    /// Whether this record asserts nothing — unassessed, untagged, no
    /// description, no attributes. Such a record carries no operator intent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.applies_to.is_none()
            && self.tags.is_empty()
            && self.description.is_none()
            && self.attributes.is_empty()
    }
}

/// Which ceph versions a patch applies to (design §3). Persisted with
/// **adjacent** serde tagging (`{"kind":…,"value":…}`) because the unit/newtype
/// variants preclude the internal tagging `Provenance` uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Applicability {
    /// Applies to any ceph version / release.
    Generic,
    /// Applies to the listed versions/lines (never stored empty — an empty set
    /// normalizes to `applies_to = None`, design §5).
    Versions(BTreeSet<VersionSpec>),
}

impl Applicability {
    /// Whether a patch with this applicability matches `query` (design §7).
    #[must_use]
    pub fn matches(&self, query: &VersionQuery) -> bool {
        match self {
            Applicability::Generic => true,
            Applicability::Versions(specs) => specs.iter().any(|s| spec_matches(s, query)),
        }
    }
}

/// One applicability target: a `major.minor` line or a full point release.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum VersionSpec {
    /// `major.minor`, e.g. `18.2` — matches any `v18.2.*`.
    Line(String),
    /// A full version, v-prefixed, e.g. `v18.2.0` (or `v18.2.0-rc1`).
    Exact(String),
}

impl VersionSpec {
    /// The `major.minor` line of this spec. For `Line` it is the value; for
    /// `Exact` it is the version's first two numeric components.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            VersionSpec::Line(l) => l.clone(),
            VersionSpec::Exact(e) => exact_line(e),
        }
    }
}

/// A parsed `--ceph-version` query or release `base_ref` (design §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionQuery {
    /// ≥ 3 numeric parts: a point release. `canon` is the v-prefixed full
    /// string (suffix included); `line` is its `major.minor`.
    Point { canon: String, line: String },
    /// Exactly 2 numeric parts: a line query (`major.minor`).
    Line { line: String },
}

/// Strip an optional leading `v` and a trailing `-prerelease` tag, then split
/// the numeric core on `.`. Returns `(parts, suffix)` where `suffix` includes
/// its leading `-` (empty if none). Errors if the core is empty or any part is
/// non-numeric (shared by §5 setting and §7 querying).
fn split_version(input: &str) -> Result<(Vec<String>, String), CrtCoreError> {
    let trimmed = input.trim();
    let no_v = trimmed.strip_prefix('v').unwrap_or(trimmed);
    // Ceph versions carry `-` only in the pre-release tag, so the first `-`
    // splits the numeric core from the suffix.
    let (core, suffix) = match no_v.split_once('-') {
        Some((c, rest)) => (c, format!("-{rest}")),
        None => (no_v, String::new()),
    };
    if core.is_empty() {
        return Err(CrtCoreError::InvalidVersion(input.to_owned()));
    }
    let parts: Vec<String> = core.split('.').map(str::to_owned).collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(CrtCoreError::InvalidVersion(input.to_owned()));
    }
    Ok((parts, suffix))
}

/// `major.minor` of a canonical exact string like `v18.2.0` or `v18.2.0-rc1`.
/// `e` is produced by [`parse_version_spec`], so the re-parse is infallible;
/// fall back to the whole string defensively.
fn exact_line(e: &str) -> String {
    match split_version(e) {
        Ok((parts, _)) if parts.len() >= 2 => format!("{}.{}", parts[0], parts[1]),
        _ => e.to_owned(),
    }
}

/// Parse a `--ceph-version` value into a [`VersionSpec`] (design §5): ≥ 3
/// numeric parts ⟹ `Exact` (v-prefixed, suffix re-attached); exactly 2 ⟹
/// `Line` (bare); fewer, or a pre-release tag on a line, is an error.
pub fn parse_version_spec(input: &str) -> Result<VersionSpec, CrtCoreError> {
    let (parts, suffix) = split_version(input)?;
    match parts.len() {
        2 if suffix.is_empty() => Ok(VersionSpec::Line(format!("{}.{}", parts[0], parts[1]))),
        n if n >= 3 => Ok(VersionSpec::Exact(format!(
            "v{}{}",
            parts.join("."),
            suffix
        ))),
        _ => Err(CrtCoreError::InvalidVersion(input.to_owned())),
    }
}

/// Parse a `--ceph-version` query / release `base_ref` into a [`VersionQuery`]
/// (design §7): ≥ 3 numeric parts ⟹ a point query; exactly 2 ⟹ a line query;
/// fewer, or a pre-release tag on a line, is an error.
pub fn parse_version_query(input: &str) -> Result<VersionQuery, CrtCoreError> {
    let (parts, suffix) = split_version(input)?;
    match parts.len() {
        2 if suffix.is_empty() => Ok(VersionQuery::Line {
            line: format!("{}.{}", parts[0], parts[1]),
        }),
        n if n >= 3 => Ok(VersionQuery::Point {
            canon: format!("v{}{}", parts.join("."), suffix),
            line: format!("{}.{}", parts[0], parts[1]),
        }),
        _ => Err(CrtCoreError::InvalidVersion(input.to_owned())),
    }
}

/// Whether one `spec` matches `query` per the design §7 table.
fn spec_matches(spec: &VersionSpec, query: &VersionQuery) -> bool {
    match (spec, query) {
        (VersionSpec::Line(l), VersionQuery::Point { line, .. }) => l == line,
        (VersionSpec::Line(l), VersionQuery::Line { line }) => l == line,
        (VersionSpec::Exact(e), VersionQuery::Point { canon, .. }) => e == canon,
        (VersionSpec::Exact(e), VersionQuery::Line { line }) => &exact_line(e) == line,
    }
}

/// Whether `applies_to` matches `query`. `None` (unassessed) never matches —
/// it is never assumed generic (design §7 / §9).
#[must_use]
pub fn applies_to_matches(applies_to: &Option<Applicability>, query: &VersionQuery) -> bool {
    applies_to.as_ref().is_some_and(|a| a.matches(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(specs: &[VersionSpec]) -> Applicability {
        Applicability::Versions(specs.iter().cloned().collect())
    }

    #[test]
    fn parse_spec_classifies_by_part_count() {
        assert_eq!(
            parse_version_spec("18.2").unwrap(),
            VersionSpec::Line("18.2".to_owned())
        );
        // A point release ⟹ Exact, re-prefixed with `v`.
        assert_eq!(
            parse_version_spec("18.2.0").unwrap(),
            VersionSpec::Exact("v18.2.0".to_owned())
        );
        assert_eq!(
            parse_version_spec("v18.2.0").unwrap(),
            VersionSpec::Exact("v18.2.0".to_owned())
        );
        // A pre-release tag is kept on Exact.
        assert_eq!(
            parse_version_spec("v18.2.0-rc1").unwrap(),
            VersionSpec::Exact("v18.2.0-rc1".to_owned())
        );
        // A sub-patch component (4 parts) is still Exact, and its line is maj.min.
        let sub = parse_version_spec("18.2.0.1").unwrap();
        assert_eq!(sub, VersionSpec::Exact("v18.2.0.1".to_owned()));
        assert_eq!(sub.line(), "18.2");
    }

    #[test]
    fn parse_spec_rejects_bad_inputs() {
        for bad in ["18", "", "v", "18.x", "18..2", "18.2-rc1", "  "] {
            assert!(
                parse_version_spec(bad).is_err(),
                "expected error for {bad:?}"
            );
        }
    }

    #[test]
    fn parse_query_classifies_point_vs_line() {
        assert_eq!(
            parse_version_query("v18.2.1").unwrap(),
            VersionQuery::Point {
                canon: "v18.2.1".to_owned(),
                line: "18.2".to_owned()
            }
        );
        assert_eq!(
            parse_version_query("18.2").unwrap(),
            VersionQuery::Line {
                line: "18.2".to_owned()
            }
        );
        assert_eq!(
            parse_version_query("v18.2.0-rc1").unwrap(),
            VersionQuery::Point {
                canon: "v18.2.0-rc1".to_owned(),
                line: "18.2".to_owned()
            }
        );
        for bad in ["18", "18.2-rc1", "nope"] {
            assert!(parse_version_query(bad).is_err(), "want err for {bad:?}");
        }
    }

    #[test]
    fn matching_table_covers_point_and_line_queries() {
        // (spec, query, expected) — the design §7 truth table, including the
        // two named edge cases (point must not match a differing suffix; a
        // line query matches an Exact on the same line).
        let cases: &[(VersionSpec, &str, bool)] = &[
            (VersionSpec::Line("18.2".to_owned()), "v18.2.1", true),
            (VersionSpec::Line("18.2".to_owned()), "18.2", true),
            (VersionSpec::Line("18.2".to_owned()), "v18.3.0", false),
            (VersionSpec::Line("18.2".to_owned()), "18.3", false),
            (VersionSpec::Exact("v18.2.1".to_owned()), "v18.2.1", true),
            // suffix differs ⇒ no point match, either direction.
            (
                VersionSpec::Exact("v18.2.0".to_owned()),
                "v18.2.0-rc1",
                false,
            ),
            (
                VersionSpec::Exact("v18.2.0-rc1".to_owned()),
                "v18.2.0",
                false,
            ),
            // a line query matches an Exact on the same line.
            (VersionSpec::Exact("v18.2.1".to_owned()), "18.2", true),
            (VersionSpec::Exact("v18.2.1".to_owned()), "18.3", false),
            (VersionSpec::Exact("v18.2.0".to_owned()), "v18.2.1", false),
        ];
        for (spec, query, expected) in cases {
            let q = parse_version_query(query).expect("valid query");
            assert_eq!(
                versions(std::slice::from_ref(spec)).matches(&q),
                *expected,
                "spec={spec:?} query={query}"
            );
        }
    }

    #[test]
    fn generic_matches_any_and_none_matches_nothing() {
        let point = parse_version_query("v18.2.1").unwrap();
        let line = parse_version_query("19.2").unwrap();
        assert!(Applicability::Generic.matches(&point));
        assert!(Applicability::Generic.matches(&line));

        // None (unassessed) is never treated as generic.
        let none: Option<Applicability> = None;
        assert!(!applies_to_matches(&none, &point));
        assert!(!applies_to_matches(&none, &line));

        // An empty Versions set matches nothing.
        assert!(!versions(&[]).matches(&point));

        let some = Some(versions(&[VersionSpec::Line("18.2".to_owned())]));
        assert!(applies_to_matches(&some, &point));
        assert!(!applies_to_matches(&some, &line));
    }

    #[test]
    fn annotations_round_trip_through_json() {
        let mut ann = PatchAnnotations {
            applies_to: Some(versions(&[
                VersionSpec::Line("18.2".to_owned()),
                VersionSpec::Exact("v18.2.0".to_owned()),
            ])),
            description: Some("backport of the rgw fix".to_owned()),
            ..Default::default()
        };
        ann.tags.insert("rgw".to_owned());
        ann.tags.insert("clyso/tunables".to_owned());
        ann.attributes
            .insert("retire-when".to_owned(), "v18.3.0".to_owned());

        let json = serde_json::to_string_pretty(&ann).unwrap();
        let back: PatchAnnotations = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ann);
    }

    #[test]
    fn applicability_uses_adjacent_tagging() {
        // Generic is a bare tag; Versions/Line/Exact carry a `value`.
        let generic = serde_json::to_value(Applicability::Generic).unwrap();
        assert_eq!(generic, serde_json::json!({ "kind": "generic" }));

        let line = serde_json::to_value(VersionSpec::Line("18.2".to_owned())).unwrap();
        assert_eq!(line, serde_json::json!({ "kind": "line", "value": "18.2" }));

        let exact = serde_json::to_value(VersionSpec::Exact("v18.2.0".to_owned())).unwrap();
        assert_eq!(
            exact,
            serde_json::json!({ "kind": "exact", "value": "v18.2.0" })
        );

        // A Versions set round-trips containing both kinds.
        let v = versions(&[
            VersionSpec::Line("18.2".to_owned()),
            VersionSpec::Exact("v18.2.0".to_owned()),
        ]);
        let back: Applicability =
            serde_json::from_value(serde_json::to_value(&v).unwrap()).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn empty_record_reports_empty() {
        assert!(PatchAnnotations::default().is_empty());
        let mut ann = PatchAnnotations::default();
        ann.tags.insert("rgw".to_owned());
        assert!(!ann.is_empty());
    }
}
