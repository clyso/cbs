// CRT core — the in-tree materialization record (design §3/§8).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The public-safe verification bundle's typed contents (design §3/§8): the
//! `MaterializationRecord` written to `000-RELEASE/record.json` and signed by
//! its detached `record.json.asc`, plus the pure `source_tree_digest` combine
//! over a materialized source tree. These carry **no** `visibility` /
//! `justification.internal` — the in-tree record is public-safe by construction
//! (§3), so a downstream consumer can never recover classification from it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ManifestEntry, Provenance, RenderSpec, Sha256};

/// Schema version of the [`MaterializationRecord`] (design addition, plan M4
/// decision 5 / v1 plan-review F2). Independent of `crate::SCHEMA_VERSION` (the
/// sealed manifest's); bumped only when the in-tree record's shape changes.
pub const MATERIALIZATION_RECORD_VERSION: u32 = 1;

/// One materialized patch in the in-tree record's BOM (design §3). Public-safe:
/// the byte-exact `blob_hash` address, the offset-invariant `patch_id`, the
/// apply `order`, and the `git_commit` it landed as on the branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedPatch {
    pub order: u32,
    pub blob_hash: Sha256,
    pub patch_id: String,
    pub git_commit: String,
}

/// The in-tree, public-safe verification record (design §3/§8), written to
/// `000-RELEASE/record.json` and signed by the detached `record.json.asc`. It
/// back-references the sealed release by digest, pins the rendering inputs,
/// records the `source_tree_digest` over the materialized source, and carries a
/// `bundle_digests` map covering **every other** `000-RELEASE/` file — so the
/// single detached signature transitively covers the whole bundle. It carries
/// **no** `visibility` or `justification.internal`: public-safe by construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializationRecord {
    pub schema_version: u32,
    /// Back-reference to the sealed release (opaque to outside consumers).
    pub s3_manifest_digest: Sha256,
    pub base_ref: String,
    /// When this record was materialized (RFC 3339), sourced at materialize
    /// time (v1 plan-review F11).
    pub created: String,
    pub render: RenderSpec,
    /// Hash over the materialized source files, excluding `000-RELEASE/` (§8).
    pub source_tree_digest: Sha256,
    /// `sha256` of **every other** `000-RELEASE/` file, keyed by basename — so
    /// the signed record covers the whole bundle by construction (§8).
    pub bundle_digests: BTreeMap<String, Sha256>,
    pub patches: Vec<MaterializedPatch>,
}

/// The public-safe per-patch provenance written to `000-RELEASE/provenance.json`
/// (design §8). A typed projection of the sealed manifest entries — **not** a
/// raw `PatchMeta` dump — so the public bundle cannot leak downstream
/// classification by omission: it carries the apply `order`, the byte-exact
/// `blob_hash`, the offset-invariant `patch_id`, and the public `provenance`
/// origin (upstream PRs/commits, or a free-text downstream description), and
/// **never** the entry's `visibility` or `justification.internal`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicProvenance {
    pub patches: Vec<PublicPatchProvenance>,
}

/// One patch's public origin facts in [`PublicProvenance`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicPatchProvenance {
    pub order: u32,
    pub blob_hash: Sha256,
    pub patch_id: String,
    pub provenance: Provenance,
}

impl PublicProvenance {
    /// Project the public-safe provenance from the sealed manifest entries,
    /// sorted by `order` for stable bytes. The projection is field-explicit (it
    /// copies only the four public fields), so a future classification field on
    /// `ManifestEntry` cannot silently flow into the public bundle.
    #[must_use]
    pub fn from_entries(entries: &[ManifestEntry]) -> Self {
        let mut patches: Vec<PublicPatchProvenance> = entries
            .iter()
            .map(|e| PublicPatchProvenance {
                order: e.order,
                blob_hash: e.blob_hash,
                patch_id: e.patch_id.clone(),
                provenance: e.provenance.clone(),
            })
            .collect();
        patches.sort_by_key(|p| p.order);
        Self { patches }
    }
}

/// The deterministic digest over a materialized source tree (design §8/§14):
/// `sha256` of a canonical serialization of the sorted `(path, content-hash)`
/// pairs — one per non-excluded file. `files` maps each file's repo-relative
/// slash path to a **domain-separated** hash of its bytes (regular file) or its
/// symlink target (so a file↔symlink swap cannot collide); the caller (the
/// `crt` walker) builds the map and applies that tagging. `BTreeMap` iteration
/// is sorted, so the serialization — and thus the digest — is canonical.
///
/// Each pair is framed `<path>\0<hex>\n`. A path cannot contain `\0` (POSIX),
/// so the `\0` unambiguously bounds the path even when it contains a newline.
#[must_use]
pub fn source_tree_digest(files: &BTreeMap<String, Sha256>) -> Sha256 {
    let mut buf = Vec::new();
    for (path, hash) in files {
        buf.extend_from_slice(path.as_bytes());
        buf.push(0);
        buf.extend_from_slice(hash.to_hex().as_bytes());
        buf.push(b'\n');
    }
    Sha256::of(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_tree_digest_is_independent_of_insertion_order() {
        // BTreeMap canonicalizes order, so insertion order cannot move the
        // digest.
        let a = Sha256::of(b"a");
        let b = Sha256::of(b"b");
        let mut m1 = BTreeMap::new();
        m1.insert("z.txt".to_owned(), a);
        m1.insert("a.txt".to_owned(), b);
        let mut m2 = BTreeMap::new();
        m2.insert("a.txt".to_owned(), b);
        m2.insert("z.txt".to_owned(), a);
        assert_eq!(source_tree_digest(&m1), source_tree_digest(&m2));
    }

    #[test]
    fn source_tree_digest_distinguishes_path_and_content() {
        let h = Sha256::of(b"x");
        let mut base = BTreeMap::new();
        base.insert("a.txt".to_owned(), h);
        // Same content hash at a different path → different digest.
        let mut moved = BTreeMap::new();
        moved.insert("b.txt".to_owned(), h);
        assert_ne!(source_tree_digest(&base), source_tree_digest(&moved));
        // Different content hash at the same path → different digest.
        let mut changed = BTreeMap::new();
        changed.insert("a.txt".to_owned(), Sha256::of(b"y"));
        assert_ne!(source_tree_digest(&base), source_tree_digest(&changed));
    }

    #[test]
    fn public_provenance_projects_public_fields_only_sorted_by_order() {
        use crate::{
            Blast, Conflict, Coverage, Justification, JustificationKind, Lifecycle, ManifestEntry,
            PatchStatus, Provenance, Risk, UpstreamPrState, Visibility,
        };

        // Two entries out of `order`, one carrying an internal justification and
        // a non-public visibility — neither may reach the public projection.
        let entry = |order: u32, blob: &[u8], pid: &str| ManifestEntry {
            blob_hash: Sha256::of(blob),
            patch_id: pid.to_owned(),
            order,
            visibility: Visibility::Private,
            category: "fix".to_owned(),
            risk: Risk {
                component: "rgw".to_owned(),
                blast: Blast::Availability,
                conflict: Conflict::Trivial,
                coverage: Coverage::Partial,
            },
            justification: Justification {
                kind: JustificationKind::Engineering,
                refs: vec![],
                public_summary: "public".to_owned(),
                internal: Some("SECRET-INTERNAL-NOTE".to_owned()),
            },
            behavior_change: None,
            upgrade_notes: None,
            lifecycle: Lifecycle {
                status: PatchStatus::Active,
                first_shipped_in: None,
            },
            data_structure_change: None,
            provenance: Provenance::UpstreamPr {
                prs: vec!["https://github.com/ceph/ceph/pull/1".to_owned()],
                commits: vec!["deadbeef".to_owned()],
                state: UpstreamPrState::MergedMain,
            },
        };
        let entries = vec![entry(2, b"two", "pid-2"), entry(1, b"one", "pid-1")];

        let prov = PublicProvenance::from_entries(&entries);

        // Sorted by `order`, regardless of the input order.
        assert_eq!(
            prov.patches.iter().map(|p| p.order).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(prov.patches[0].patch_id, "pid-1");

        // Public-safe: the serialized projection names no classification field.
        let json = serde_json::to_string(&prov).unwrap();
        assert!(!json.contains("visibility"), "{json}");
        assert!(!json.contains("internal"), "{json}");
        assert!(!json.contains("SECRET-INTERNAL-NOTE"), "{json}");
        assert!(!json.contains("public_summary"), "{json}");
        // …but it does carry the public origin facts.
        assert!(json.contains("pid-1"));
        assert!(json.contains("ceph/ceph/pull/1"));
    }

    #[test]
    fn record_round_trips_through_json_and_is_public_safe() {
        let mut bundle = BTreeMap::new();
        bundle.insert("sbom.cdx.json".to_owned(), Sha256::of(b"sbom"));
        let record = MaterializationRecord {
            schema_version: MATERIALIZATION_RECORD_VERSION,
            s3_manifest_digest: Sha256::of(b"manifest"),
            base_ref: "v18.2.0".to_owned(),
            created: "2026-06-25T00:00:00+00:00".to_owned(),
            render: RenderSpec {
                minijinja_version: "2.21.0".to_owned(),
                template_digest: Sha256::of(b"template"),
            },
            source_tree_digest: Sha256::of(b"tree"),
            bundle_digests: bundle,
            patches: vec![MaterializedPatch {
                order: 1,
                blob_hash: Sha256::of(b"blob"),
                patch_id: "pid-1".to_owned(),
                git_commit: "abc123".to_owned(),
            }],
        };
        let json = serde_json::to_vec(&record).unwrap();
        let back: MaterializationRecord = serde_json::from_slice(&json).unwrap();
        assert_eq!(record, back);
        // Public-safe: the serialized form names no classification field.
        let text = String::from_utf8(json).unwrap();
        assert!(!text.contains("visibility"));
        assert!(!text.contains("internal"));
    }
}
