// crt — applying operator annotations to patches.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Maps annotation CLI flags to the design §5 state transitions and merges
//! them into a blob's annotations record. Holds the pure transition logic
//! (the §9 "no silent applicability claims" invariant lives here, not in the
//! pure `crt-core` types) plus the store read-modify-write. This commit wires
//! up the bulk-at-import path; the per-patch `annotate` command and the extra
//! transitions it needs (remove, unassess, …) land in a later commit.

use anyhow::{Result, bail};
use crt_core::{Applicability, PatchAnnotations, Sha256, VersionSpec, parse_version_spec};
use crt_store::Store;

/// The applicability half of a bulk annotation (design §5.1) — `--ceph-version`
/// and `--generic` are mutually exclusive at the CLI.
pub enum BulkApplies {
    Generic,
    Version(VersionSpec),
}

/// A bulk annotation applied to every patch of an import: an optional
/// applicability assertion plus tags to add. Tag-only is valid.
pub struct BulkAnnotation {
    pub applies: Option<BulkApplies>,
    pub tags: Vec<String>,
}

impl BulkAnnotation {
    /// Build from import flags, returning `None` when none were given — a
    /// flag-less import touches no annotations (§5.1). `--ceph-version` is
    /// parsed/validated up front so a bad version fails before any import work.
    pub fn from_flags(
        ceph_version: Option<&str>,
        generic: bool,
        tags: Vec<String>,
    ) -> Result<Option<Self>> {
        let applies = match (ceph_version, generic) {
            // clap's `conflicts_with` already rejects this; guard defensively.
            (Some(_), true) => bail!("--ceph-version and --generic are mutually exclusive"),
            (Some(v), false) => Some(BulkApplies::Version(parse_version_spec(v)?)),
            (None, true) => Some(BulkApplies::Generic),
            (None, false) => None,
        };
        if applies.is_none() && tags.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self { applies, tags }))
    }
}

/// Add a version to `applies_to` (design §5): unassessed → that single version;
/// an existing version set gains it; `Generic` absorbs it — a no-op reported as
/// `true` so the caller can warn.
fn add_version(
    applies_to: Option<Applicability>,
    spec: VersionSpec,
) -> (Option<Applicability>, bool) {
    match applies_to {
        None => (
            Some(Applicability::Versions([spec].into_iter().collect())),
            false,
        ),
        Some(Applicability::Versions(mut specs)) => {
            specs.insert(spec);
            (Some(Applicability::Versions(specs)), false)
        }
        Some(Applicability::Generic) => (Some(Applicability::Generic), true),
    }
}

/// Merge a bulk annotation into `ann` (design §5.1), never clobbering existing
/// tags/description/attributes (§9). Returns the merged record and whether a
/// `--ceph-version` was absorbed by an existing `Generic` (a warning).
fn merge_bulk(mut ann: PatchAnnotations, bulk: &BulkAnnotation) -> (PatchAnnotations, bool) {
    let mut absorbed = false;
    match &bulk.applies {
        Some(BulkApplies::Generic) => ann.applies_to = Some(Applicability::Generic),
        Some(BulkApplies::Version(spec)) => {
            let (next, warn) = add_version(ann.applies_to.take(), spec.clone());
            ann.applies_to = next;
            absorbed = warn;
        }
        None => {}
    }
    for t in &bulk.tags {
        ann.tags.insert(t.clone());
    }
    (ann, absorbed)
}

/// Apply a bulk annotation to every blob in `hashes`, merging into each blob's
/// existing record (design §5.1; §9 — import never clobbers). A blob with no
/// record starts from the default. Returns the blobs where a `--ceph-version`
/// was absorbed by an existing `Generic`, for a caller warning.
pub async fn apply_bulk(
    store: &dyn Store,
    hashes: &[Sha256],
    bulk: &BulkAnnotation,
) -> Result<Vec<Sha256>> {
    let mut absorbed = Vec::new();
    for hash in hashes {
        let current = store.get_annotations(hash).await?.unwrap_or_default();
        let (merged, warn) = merge_bulk(current, bulk);
        store.put_annotations(hash, &merged).await?;
        if warn {
            absorbed.push(*hash);
        }
    }
    Ok(absorbed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt_core::blob_hash;
    use crt_store::ObjectBackedStore;

    fn versions(specs: &[VersionSpec]) -> Applicability {
        Applicability::Versions(specs.iter().cloned().collect())
    }

    #[test]
    fn add_version_transitions() {
        let l = VersionSpec::Line("18.2".to_owned());
        let (a, warn) = add_version(None, l.clone());
        assert!(!warn);
        assert_eq!(a, Some(versions(std::slice::from_ref(&l))));

        // A second version joins the set.
        let e = VersionSpec::Exact("v18.2.0".to_owned());
        let (a2, warn2) = add_version(a, e.clone());
        assert!(!warn2);
        assert_eq!(a2, Some(versions(&[l, e])));

        // Generic absorbs the version and warns (no-op).
        let (g, warn3) = add_version(
            Some(Applicability::Generic),
            VersionSpec::Line("19.2".to_owned()),
        );
        assert!(warn3);
        assert_eq!(g, Some(Applicability::Generic));
    }

    #[test]
    fn from_flags_parses_and_guards() {
        // No flags ⇒ nothing to do.
        assert!(
            BulkAnnotation::from_flags(None, false, vec![])
                .unwrap()
                .is_none()
        );
        // Tag-only is valid (no applicability).
        let t = BulkAnnotation::from_flags(None, false, vec!["rgw".to_owned()])
            .unwrap()
            .unwrap();
        assert!(t.applies.is_none());
        assert_eq!(t.tags, vec!["rgw".to_owned()]);
        // Generic / a version.
        assert!(matches!(
            BulkAnnotation::from_flags(None, true, vec![])
                .unwrap()
                .unwrap()
                .applies,
            Some(BulkApplies::Generic)
        ));
        assert!(matches!(
            BulkAnnotation::from_flags(Some("18.2"), false, vec![])
                .unwrap()
                .unwrap()
                .applies,
            Some(BulkApplies::Version(VersionSpec::Line(_)))
        ));
        // Mutually exclusive, and a bad version, both error.
        assert!(BulkAnnotation::from_flags(Some("18.2"), true, vec![]).is_err());
        assert!(BulkAnnotation::from_flags(Some("nope"), false, vec![]).is_err());
    }

    #[test]
    fn merge_preserves_existing_facets() {
        let mut existing = PatchAnnotations {
            description: Some("keep me".to_owned()),
            ..Default::default()
        };
        existing.tags.insert("existing".to_owned());
        let bulk = BulkAnnotation {
            applies: Some(BulkApplies::Version(VersionSpec::Line("18.2".to_owned()))),
            tags: vec!["rgw".to_owned()],
        };
        let (merged, warn) = merge_bulk(existing, &bulk);
        assert!(!warn);
        assert_eq!(merged.description.as_deref(), Some("keep me"));
        assert!(merged.tags.contains("existing"));
        assert!(merged.tags.contains("rgw"));
        assert!(matches!(
            merged.applies_to,
            Some(Applicability::Versions(_))
        ));
    }

    #[tokio::test]
    async fn apply_bulk_merges_and_warns_on_generic() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"p");
        // A pre-existing Generic record with a tag.
        let mut pre = PatchAnnotations {
            applies_to: Some(Applicability::Generic),
            ..Default::default()
        };
        pre.tags.insert("old".to_owned());
        store.put_annotations(&h, &pre).await.unwrap();

        let bulk = BulkAnnotation {
            applies: Some(BulkApplies::Version(VersionSpec::Line("18.2".to_owned()))),
            tags: vec!["new".to_owned()],
        };
        let absorbed = apply_bulk(&store, &[h], &bulk).await.unwrap();
        assert_eq!(absorbed, vec![h], "version absorbed by Generic warns");

        let got = store.get_annotations(&h).await.unwrap().unwrap();
        assert_eq!(got.applies_to, Some(Applicability::Generic));
        assert!(got.tags.contains("old"), "existing tag preserved");
        assert!(got.tags.contains("new"), "new tag merged");
    }

    #[tokio::test]
    async fn apply_bulk_sets_versions_on_unassessed() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"q");
        let bulk = BulkAnnotation {
            applies: Some(BulkApplies::Version(VersionSpec::Exact(
                "v18.2.0".to_owned(),
            ))),
            tags: vec![],
        };
        let absorbed = apply_bulk(&store, &[h], &bulk).await.unwrap();
        assert!(absorbed.is_empty());
        let got = store.get_annotations(&h).await.unwrap().unwrap();
        assert_eq!(
            got.applies_to,
            Some(versions(&[VersionSpec::Exact("v18.2.0".to_owned())]))
        );
    }
}
