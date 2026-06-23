// CRT core — deterministic CycloneDX SBOM (design §7.1).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Project a sealed [`Manifest`] into one CycloneDX 1.6 JSON document — a pure
//! function of the manifest (design §7.1), so `verify` leg 4 (M4) can re-derive
//! it and byte-compare. Determinism is the contract:
//!
//! - `serialNumber` is derived from the manifest digest (not a random UUID);
//! - `metadata.timestamp` is `release.created` (not the wall clock);
//! - serde struct field order + `Vec` (never a map) fix the byte order.
//!
//! The Ceph component is one `component`; each patch is an entry under its
//! `pedigree.patches[]` (`{ type: "backport", diff, resolves }`). The diff
//! records the patch's content address (the manifest stores the patch by hash,
//! not inline); `resolves` carries the justification. `justification.internal`
//! is never emitted — only `public_summary` and `refs`.

use serde::Serialize;

use crate::{CrtCoreError, JustificationKind, Manifest, ManifestEntry, digest};

/// The CycloneDX spec version this projection targets.
const SPEC_VERSION: &str = "1.6";

/// Build the CycloneDX SBOM for `manifest` as pretty-printed JSON. Pure and
/// deterministic: the same manifest always yields byte-identical output.
pub fn build_sbom(manifest: &Manifest) -> Result<String, CrtCoreError> {
    let release = &manifest.release;
    let bom = Bom {
        bom_format: "CycloneDX",
        spec_version: SPEC_VERSION,
        serial_number: serial_number(manifest)?,
        version: 1,
        metadata: Metadata {
            timestamp: release.created.clone(),
            component: MetadataComponent {
                kind: "application",
                bom_ref: release.name.clone(),
                name: format!("{} {}", release.product, release.name),
                version: release.name.clone(),
            },
        },
        components: vec![Component {
            kind: "application",
            bom_ref: format!("{}@{}", release.product, release.base_ref),
            name: release.product.clone(),
            version: release.base_ref.clone(),
            pedigree: Pedigree {
                patches: manifest.entries.iter().map(patch_for_entry).collect(),
            },
        }],
    };
    // Byte-determinism also rests on serde_json's pretty-printer being stable
    // across versions (it is, de facto — but unpinned, unlike the minijinja
    // version on the notes side). M4's verify leg 4 byte-compares this output,
    // so pin serde_json there if leg 4 ever observes drift.
    serde_json::to_string_pretty(&bom).map_err(|e| CrtCoreError::Sbom(e.to_string()))
}

/// A `urn:uuid:` serial derived from the manifest digest — deterministic, so the
/// SBOM never carries a random UUID (design §7.1). The first 32 nibbles of the
/// sha256 are formatted in the canonical 8-4-4-4-12 UUID grouping.
fn serial_number(manifest: &Manifest) -> Result<String, CrtCoreError> {
    let hex = digest(manifest)?.to_hex();
    Ok(format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32],
    ))
}

fn patch_for_entry(entry: &ManifestEntry) -> Patch {
    Patch {
        kind: "backport",
        diff: Diff {
            text: DiffText {
                content: format!(
                    "patch_id={} blob_sha256={}",
                    entry.patch_id, entry.blob_hash
                ),
            },
        },
        resolves: vec![Issue {
            kind: issue_kind(entry.justification.kind),
            name: entry.justification.public_summary.clone(),
            references: entry.justification.refs.clone(),
        }],
    }
}

/// Map a downstream justification to a CycloneDX issue type.
fn issue_kind(kind: JustificationKind) -> &'static str {
    match kind {
        JustificationKind::Cve => "security",
        JustificationKind::Customer => "defect",
        JustificationKind::Engineering => "enhancement",
    }
}

// --- CycloneDX 1.6 wire structs (only the fields this projection emits). Field
// declaration order is the JSON key order — load-bearing for determinism. ---

#[derive(Serialize)]
struct Bom {
    #[serde(rename = "bomFormat")]
    bom_format: &'static str,
    #[serde(rename = "specVersion")]
    spec_version: &'static str,
    #[serde(rename = "serialNumber")]
    serial_number: String,
    version: u32,
    metadata: Metadata,
    components: Vec<Component>,
}

#[derive(Serialize)]
struct Metadata {
    timestamp: String,
    component: MetadataComponent,
}

#[derive(Serialize)]
struct MetadataComponent {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "bom-ref")]
    bom_ref: String,
    name: String,
    version: String,
}

#[derive(Serialize)]
struct Component {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "bom-ref")]
    bom_ref: String,
    name: String,
    version: String,
    pedigree: Pedigree,
}

#[derive(Serialize)]
struct Pedigree {
    patches: Vec<Patch>,
}

#[derive(Serialize)]
struct Patch {
    #[serde(rename = "type")]
    kind: &'static str,
    diff: Diff,
    resolves: Vec<Issue>,
}

#[derive(Serialize)]
struct Diff {
    text: DiffText,
}

#[derive(Serialize)]
struct DiffText {
    content: String,
}

#[derive(Serialize)]
struct Issue {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    references: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Blast, Branding, Conflict, Coverage, Identity, Justification, KnownIssue, Lifecycle,
        PatchStatus, Provenance, ReleaseHeader, RenderSpec, Risk, SCHEMA_VERSION, UpstreamPrState,
        Visibility, blob_hash,
    };

    fn entry(order: u32, kind: JustificationKind, summary: &str, refs: &[&str]) -> ManifestEntry {
        ManifestEntry {
            blob_hash: blob_hash(format!("patch-{order}").as_bytes()),
            patch_id: format!("pid-{order}"),
            order,
            visibility: Visibility::Public,
            category: "fix".to_owned(),
            risk: Risk {
                component: "rgw".to_owned(),
                blast: Blast::Availability,
                conflict: Conflict::Trivial,
                coverage: Coverage::Partial,
            },
            justification: Justification {
                kind,
                refs: refs.iter().map(|r| (*r).to_owned()).collect(),
                public_summary: summary.to_owned(),
                internal: Some("DO-NOT-LEAK-internal".to_owned()),
            },
            behavior_change: None,
            upgrade_notes: None,
            lifecycle: Lifecycle {
                status: PatchStatus::Active,
                first_shipped_in: None,
            },
            data_structure_change: None,
            provenance: Provenance::UpstreamPr {
                prs: vec![],
                commits: vec![],
                state: UpstreamPrState::MergedMain,
            },
        }
    }

    fn manifest(entries: Vec<ManifestEntry>) -> Manifest {
        Manifest {
            schema_version: SCHEMA_VERSION,
            release: ReleaseHeader {
                product: "ceph".to_owned(),
                namespace: "clyso-enterprise".to_owned(),
                channel: "ces".to_owned(),
                name: "ces-v18.2.0".to_owned(),
                base_ref: "v18.2.0".to_owned(),
                created: "2026-06-21T00:00:00+00:00".to_owned(),
                author: Identity {
                    name: "Releaser".to_owned(),
                    email: "rel@example.com".to_owned(),
                },
            },
            entries,
            known_issues: vec![KnownIssue {
                summary: "A known issue.".to_owned(),
                refs: vec![],
            }],
            upgrade_notes: None,
            branding: Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "blurb".to_owned(),
                footer: "footer".to_owned(),
            },
            render: RenderSpec {
                minijinja_version: crate::RENDER_MINIJINJA_VERSION.to_owned(),
                template_digest: blob_hash(b"template"),
            },
        }
    }

    #[test]
    fn is_deterministic_and_pure() {
        // The leg-4 contract: same manifest in, byte-identical SBOM out.
        let m = manifest(vec![
            entry(
                1,
                JustificationKind::Cve,
                "Fixes a CVE.",
                &["CVE-2026-0001"],
            ),
            entry(2, JustificationKind::Engineering, "Refactor.", &[]),
        ]);
        let a = build_sbom(&m).unwrap();
        let b = build_sbom(&m).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn has_the_expected_cyclonedx_shape() {
        let m = manifest(vec![
            entry(
                1,
                JustificationKind::Cve,
                "Fixes a CVE.",
                &["CVE-2026-0001"],
            ),
            entry(2, JustificationKind::Engineering, "Refactor.", &[]),
        ]);
        let json: serde_json::Value = serde_json::from_str(&build_sbom(&m).unwrap()).unwrap();

        assert_eq!(json["bomFormat"], "CycloneDX");
        assert_eq!(json["specVersion"], "1.6");
        assert_eq!(json["version"], 1);
        assert_eq!(json["metadata"]["timestamp"], "2026-06-21T00:00:00+00:00");
        // The ceph component carries one patch per manifest entry.
        let patches = json["components"][0]["pedigree"]["patches"]
            .as_array()
            .expect("patches array");
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0]["type"], "backport");
        assert_eq!(patches[0]["resolves"][0]["type"], "security"); // CVE
        assert_eq!(patches[1]["resolves"][0]["type"], "enhancement"); // engineering
        assert_eq!(patches[0]["resolves"][0]["name"], "Fixes a CVE.");
    }

    #[test]
    fn serial_number_is_derived_from_the_digest_not_random() {
        let m = manifest(vec![entry(1, JustificationKind::Cve, "x", &[])]);
        let hex = digest(&m).unwrap().to_hex();
        let json: serde_json::Value = serde_json::from_str(&build_sbom(&m).unwrap()).unwrap();
        let serial = json["serialNumber"].as_str().unwrap();
        assert!(serial.starts_with("urn:uuid:"));
        // The serial is the first 32 nibbles of the digest, regrouped.
        assert!(serial.contains(&hex[0..8]));
        assert!(serial.contains(&hex[20..32]));
    }

    #[test]
    fn never_emits_the_internal_note() {
        let m = manifest(vec![entry(1, JustificationKind::Customer, "Public.", &[])]);
        let out = build_sbom(&m).unwrap();
        assert!(out.contains("Public."));
        assert!(!out.contains("DO-NOT-LEAK-internal"));
        // The patch's content address is recorded in the diff.
        assert!(out.contains("blob_sha256="));
    }

    #[test]
    fn empty_entries_yield_an_empty_patch_array() {
        // A release carrying only known-issues has no entries; `pedigree.patches`
        // must serialize to `[]` and the projection stays deterministic.
        let m = manifest(vec![]);
        let json: serde_json::Value = serde_json::from_str(&build_sbom(&m).unwrap()).unwrap();
        let patches = json["components"][0]["pedigree"]["patches"]
            .as_array()
            .expect("patches array");
        assert!(patches.is_empty());
        assert_eq!(build_sbom(&m).unwrap(), build_sbom(&m).unwrap());
    }

    /// A committed byte-golden for a fixed manifest. Unlike
    /// `is_deterministic_and_pure` (same-process equality), this pins the *exact*
    /// bytes across builds, so a serialization drift — e.g. a `serde_json`
    /// pretty-printer change, which `build_sbom` flags — breaks CI rather than
    /// silently shifting the artifact `verify` leg 4 (M4) byte-compares. Mirrors
    /// `manifest.rs`'s `CANONICAL_GOLDEN`. If a deliberate SBOM-shape change
    /// fails this, re-pin it (a committed-bytes contract from M4 onward).
    const GOLDEN_SBOM: &str = r#"{
  "bomFormat": "CycloneDX",
  "specVersion": "1.6",
  "serialNumber": "urn:uuid:2ccf8243-89de-cdf0-dedb-d909b1d42fea",
  "version": 1,
  "metadata": {
    "timestamp": "2026-06-21T00:00:00+00:00",
    "component": {
      "type": "application",
      "bom-ref": "ces-v18.2.0",
      "name": "ceph ces-v18.2.0",
      "version": "ces-v18.2.0"
    }
  },
  "components": [
    {
      "type": "application",
      "bom-ref": "ceph@v18.2.0",
      "name": "ceph",
      "version": "v18.2.0",
      "pedigree": {
        "patches": [
          {
            "type": "backport",
            "diff": {
              "text": {
                "content": "patch_id=pid-1 blob_sha256=8d1c3243a35e2d54669fcc40dbb22ee8a6b9a4a9a35410cc816cf9175cd79c89"
              }
            },
            "resolves": [
              {
                "type": "security",
                "name": "Fixes a CVE.",
                "references": [
                  "CVE-2026-0001"
                ]
              }
            ]
          },
          {
            "type": "backport",
            "diff": {
              "text": {
                "content": "patch_id=pid-2 blob_sha256=33df0cf768d9f425aecca844921d344a6919929ed0563f4c83bcf0ae8118839d"
              }
            },
            "resolves": [
              {
                "type": "enhancement",
                "name": "Refactor."
              }
            ]
          }
        ]
      }
    }
  ]
}"#;

    #[test]
    fn matches_the_byte_golden() {
        let m = manifest(vec![
            entry(
                1,
                JustificationKind::Cve,
                "Fixes a CVE.",
                &["CVE-2026-0001"],
            ),
            entry(2, JustificationKind::Engineering, "Refactor.", &[]),
        ]);
        assert_eq!(build_sbom(&m).unwrap(), GOLDEN_SBOM);
    }
}
