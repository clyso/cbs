// CRT core — release manifest model, risk scoring, canonical-JSON digest.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The release manifest (design §3), its risk rubric (concept §6.1), and the
//! RFC 8785 canonical-JSON digest that seals it (design §6). Pure: no IO.
//!
//! The signed body is the [`Manifest`]; the `digest` and `signature` live in
//! the [`ReleaseRecord`] envelope *around* it, so the digest never references
//! itself. Risk `total`/`band` are **computed, not stored** (so a later band
//! re-calibration never alters a signed manifest).

use serde::{Deserialize, Serialize};

use crate::{CrtCoreError, Identity, Provenance, Sha256, UpstreamPrState};

/// Current manifest schema version — the single source of truth shared by seal
/// (which stamps it) and verify (which checks it), so the two cannot drift.
pub const SCHEMA_VERSION: u32 = 1;

/// Per-patch visibility (design §1). Recorded but **inert** in the MVP — it
/// does not filter SBOM, notes, verify, or materialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Private,
}

/// Blast radius axis (concept §6.1), weight 1→3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blast {
    Cosmetic,
    Availability,
    DataLoss,
}

/// Conflict-on-apply axis (concept §6.1), weight 1→3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Conflict {
    Clean,
    Trivial,
    Substantive,
}

/// Test-coverage axis (concept §6.1), weight 1→3 (strong is lowest risk).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    Strong,
    Partial,
    Weak,
}

impl Blast {
    /// Rubric weight (concept §6.1).
    #[must_use]
    pub fn weight(self) -> u8 {
        match self {
            Blast::Cosmetic => 1,
            Blast::Availability => 2,
            Blast::DataLoss => 3,
        }
    }
}

impl Conflict {
    /// Rubric weight (concept §6.1).
    #[must_use]
    pub fn weight(self) -> u8 {
        match self {
            Conflict::Clean => 1,
            Conflict::Trivial => 2,
            Conflict::Substantive => 3,
        }
    }
}

impl Coverage {
    /// Rubric weight (concept §6.1).
    #[must_use]
    pub fn weight(self) -> u8 {
        match self {
            Coverage::Strong => 1,
            Coverage::Partial => 2,
            Coverage::Weak => 3,
        }
    }
}

/// The derived `upstream` axis weight (concept §6.1): how settled the change is
/// upstream. Merged ⇒ 1; approved-but-open ⇒ 2; in-review, declined, or
/// downstream-only ⇒ 3. (`Declined` — closed-unmerged — is treated as highest
/// risk, like a downstream-only carry.)
#[must_use]
pub fn upstream_weight(provenance: &Provenance) -> u8 {
    match provenance {
        Provenance::UpstreamPr { state, .. } => match state {
            UpstreamPrState::MergedStable | UpstreamPrState::MergedMain => 1,
            UpstreamPrState::ApprovedOpen => 2,
            UpstreamPrState::OpenInReview | UpstreamPrState::Declined => 3,
        },
        Provenance::Other { .. } => 3,
    }
}

/// The risk band a total falls into (concept §6.1). Conventions, not gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Band {
    Low,
    Medium,
    High,
}

impl Band {
    /// Classify a risk total (4–12) into a band: 4–6 low, 7–9 medium,
    /// 10–12 high (concept §6.1).
    #[must_use]
    pub fn from_total(total: u8) -> Self {
        match total {
            ..=6 => Band::Low,
            7..=9 => Band::Medium,
            _ => Band::High,
        }
    }
}

/// The three authored risk axes plus the configurable subsystem label (design
/// §1; concept §6.1). The fourth axis (`upstream`) is derived from provenance
/// and the `total`/`band` are computed — none of those are stored here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    /// Configurable subsystem label (e.g. `rgw`); validated against
    /// `risk_components` in config at authoring time, free-form here.
    pub component: String,
    pub blast: Blast,
    pub conflict: Conflict,
    pub coverage: Coverage,
}

/// Why a patch is carried (concept §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JustificationKind {
    Cve,
    Customer,
    Engineering,
}

/// The justification for a patch: a public summary (rendered into notes) and an
/// optional internal note (S3-only, never rendered or materialized).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Justification {
    pub kind: JustificationKind,
    pub refs: Vec<String>,
    pub public_summary: String,
    pub internal: Option<String>,
}

/// Cross-release status of a patch (concept §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchStatus {
    Active,
    Superseded,
    Dropped,
}

/// Cross-release lifecycle (concept §6.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub status: PatchStatus,
    /// Earliest release name that carried this patch.
    pub first_shipped_in: Option<String>,
}

/// Flags a patch that changes on-disk/wire struct versions (concept §6.4) — a
/// data-corruption hazard if upstream later diverges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStructureChange {
    pub struct_v_bump: bool,
    pub upstream_coordinated: bool,
}

/// A per-release entry referencing a stored patch by both hashes (design §3).
/// `visibility` and `justification.internal` live only here / in S3.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub blob_hash: Sha256,
    pub patch_id: String,
    pub order: u32,
    pub visibility: Visibility,
    /// Notes grouping (conventionally `security`/`feature`/`fix`/`integration`
    /// — concept §6.5); a configurable list, free-form here.
    pub category: String,
    pub risk: Risk,
    pub justification: Justification,
    pub behavior_change: Option<String>,
    pub upgrade_notes: Option<String>,
    pub lifecycle: Lifecycle,
    pub data_structure_change: Option<DataStructureChange>,
    /// Denormalized snapshot of the patch's provenance at compose time.
    pub provenance: Provenance,
}

impl ManifestEntry {
    /// Sum of the four risk axes (concept §6.1): the three authored axes plus
    /// the `upstream` axis derived from provenance. Range 4–12.
    #[must_use]
    pub fn risk_total(&self) -> u8 {
        self.risk.blast.weight()
            + self.risk.conflict.weight()
            + self.risk.coverage.weight()
            + upstream_weight(&self.provenance)
    }

    /// The risk band for this entry (concept §6.1).
    #[must_use]
    pub fn risk_band(&self) -> Band {
        Band::from_total(self.risk_total())
    }
}

/// A release-wide known issue (concept §6.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownIssue {
    pub summary: String,
    pub refs: Vec<String>,
}

/// Channel branding, snapshotted into the manifest at seal so a later config
/// edit cannot change a sealed release's rendered notes (design §7.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branding {
    pub display_name: String,
    pub blurb: String,
    pub footer: String,
}

/// Pins notes rendering so verify can reproduce it (design §7.2/§11). The
/// template bytes are stored content-addressed at `templates/sha256/<digest>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderSpec {
    pub minijinja_version: String,
    pub template_digest: Sha256,
}

/// Release-identifying header (design §3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseHeader {
    pub product: String,
    pub namespace: String,
    pub channel: String,
    pub name: String,
    pub base_ref: String,
    /// Creation time, ISO-8601 (stored as a string for canonical stability).
    pub created: String,
    pub author: Identity,
}

/// The signed release body (design §3/§6). The `digest`/`signature` are in the
/// [`ReleaseRecord`] envelope, never inside this hashed body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub release: ReleaseHeader,
    pub entries: Vec<ManifestEntry>,
    pub known_issues: Vec<KnownIssue>,
    pub upgrade_notes: Option<String>,
    pub branding: Branding,
    pub render: RenderSpec,
}

/// A detached, ASCII-armored OpenPGP signature (design §6). The signing and
/// verification logic lands in M2.2; this is the wire type the envelope holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArmoredSignature(pub String);

/// The stored envelope around a sealed manifest (design §3/§5):
/// `manifest` + its `digest` + a detached `signature`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub manifest: Manifest,
    pub digest: Sha256,
    pub signature: ArmoredSignature,
}

/// The mutable, pre-seal manifest body (the store-backed draft). `branding`,
/// `RenderSpec`, `schema_version`, `digest`, and `signature` are all added at
/// seal time, so they are absent here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub release: ReleaseHeader,
    pub entries: Vec<ManifestEntry>,
    pub known_issues: Vec<KnownIssue>,
    pub upgrade_notes: Option<String>,
}

/// Identifies a release (or draft) within the store (design §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseKey {
    pub namespace: String,
    pub channel: String,
    pub name: String,
}

/// RFC 8785 (JCS) canonical-JSON serialization of a manifest (design §6) — the
/// exact bytes that get digested and signed. Deterministic regardless of field
/// insertion order.
pub fn canonical_json(manifest: &Manifest) -> Result<Vec<u8>, CrtCoreError> {
    serde_json_canonicalizer::to_vec(manifest).map_err(|e| CrtCoreError::Canonical(e.to_string()))
}

/// `digest = sha256(canonical_json(manifest))` (design §6). The digest commits
/// to every byte of the manifest — and transitively to every referenced patch
/// blob via its `blob_hash` — without referencing itself.
pub fn digest(manifest: &Manifest) -> Result<Sha256, CrtCoreError> {
    Ok(Sha256::of(&canonical_json(manifest)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_hash;

    fn sample_entry(order: u32, provenance: Provenance) -> ManifestEntry {
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
                kind: JustificationKind::Engineering,
                refs: vec!["https://tracker.ceph.com/issues/1".to_owned()],
                public_summary: "Fixes a thing.".to_owned(),
                internal: None,
            },
            behavior_change: None,
            upgrade_notes: None,
            lifecycle: Lifecycle {
                status: PatchStatus::Active,
                first_shipped_in: None,
            },
            data_structure_change: None,
            provenance,
        }
    }

    fn sample_manifest() -> Manifest {
        let provenance = Provenance::UpstreamPr {
            prs: vec!["https://github.com/ceph/ceph/pull/1".to_owned()],
            commits: vec!["abc".to_owned()],
            state: UpstreamPrState::MergedMain,
        };
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
            entries: vec![
                sample_entry(1, provenance.clone()),
                sample_entry(
                    2,
                    Provenance::Other {
                        description: "downstream-only".to_owned(),
                    },
                ),
            ],
            known_issues: vec![KnownIssue {
                summary: "A known issue.".to_owned(),
                refs: vec![],
            }],
            upgrade_notes: Some("Upgrade carefully.".to_owned()),
            branding: Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "blurb".to_owned(),
                footer: "footer".to_owned(),
            },
            render: RenderSpec {
                minijinja_version: "2.21.0".to_owned(),
                template_digest: blob_hash(b"template"),
            },
        }
    }

    #[test]
    fn risk_weights_and_bands() {
        // availability(2) + trivial(2) + partial(2) + merged-main(1) = 7 ⇒ medium
        let e = sample_entry(
            1,
            Provenance::UpstreamPr {
                prs: vec![],
                commits: vec![],
                state: UpstreamPrState::MergedMain,
            },
        );
        assert_eq!(e.risk_total(), 7);
        assert_eq!(e.risk_band(), Band::Medium);

        // The same axes but downstream-only ⇒ upstream weight 3 ⇒ 9, still medium.
        let e2 = sample_entry(
            1,
            Provenance::Other {
                description: "d".to_owned(),
            },
        );
        assert_eq!(e2.risk_total(), 9);
        assert_eq!(e2.risk_band(), Band::Medium);

        // Band boundaries.
        assert_eq!(Band::from_total(6), Band::Low);
        assert_eq!(Band::from_total(7), Band::Medium);
        assert_eq!(Band::from_total(9), Band::Medium);
        assert_eq!(Band::from_total(10), Band::High);
        assert_eq!(Band::from_total(12), Band::High);
    }

    #[test]
    fn max_and_min_risk_totals() {
        let mut e = sample_entry(
            1,
            Provenance::Other {
                description: "d".to_owned(),
            },
        );
        e.risk = Risk {
            component: "rgw".to_owned(),
            blast: Blast::DataLoss,
            conflict: Conflict::Substantive,
            coverage: Coverage::Weak,
        };
        assert_eq!(e.risk_total(), 12); // 3+3+3+3 (downstream)
        assert_eq!(e.risk_band(), Band::High);

        e.risk = Risk {
            component: "rgw".to_owned(),
            blast: Blast::Cosmetic,
            conflict: Conflict::Clean,
            coverage: Coverage::Strong,
        };
        e.provenance = Provenance::UpstreamPr {
            prs: vec![],
            commits: vec![],
            state: UpstreamPrState::MergedStable,
        };
        assert_eq!(e.risk_total(), 4); // 1+1+1+1
        assert_eq!(e.risk_band(), Band::Low);
    }

    #[test]
    fn canonical_json_is_deterministic() {
        let m = sample_manifest();
        let a = canonical_json(&m).unwrap();
        let b = canonical_json(&m).unwrap();
        assert_eq!(a, b);
        // RFC 8785 sorts object keys: `base_ref` precedes `name` within the
        // release header in the canonical bytes.
        let text = String::from_utf8(a).unwrap();
        let base_at = text.find("\"base_ref\"").expect("base_ref present");
        let created_at = text.find("\"created\"").expect("created present");
        assert!(base_at < created_at, "keys are lexicographically sorted");
    }

    #[test]
    fn digest_is_stable_and_change_sensitive() {
        let m = sample_manifest();
        let d1 = digest(&m).unwrap();
        let d2 = digest(&m).unwrap();
        assert_eq!(d1, d2, "digest is stable");

        // Golden canonical contract (design §6). `CANONICAL_GOLDEN` is the
        // exact RFC 8785 byte sequence the fixture serializes to; `DIGEST_GOLDEN`
        // is its sha256. A drift in either means the canonicalizer or the schema
        // changed and every prior signature is invalidated — asserting the
        // string makes that drift a diagnosable byte diff, not just a hash
        // mismatch.
        let canonical = String::from_utf8(canonical_json(&m).unwrap()).unwrap();
        assert_eq!(canonical, CANONICAL_GOLDEN, "canonical JSON contract");
        assert_eq!(d1.to_hex(), DIGEST_GOLDEN);
        assert_eq!(
            Sha256::of(CANONICAL_GOLDEN.as_bytes()).to_hex(),
            DIGEST_GOLDEN,
            "the digest golden is the sha256 of the canonical golden"
        );

        // A single-field change moves the digest.
        let mut m2 = m.clone();
        m2.release.name = "ces-v18.2.1".to_owned();
        assert_ne!(digest(&m2).unwrap(), d1);
    }

    // Captured from `canonical_json(sample_manifest())` — the exact RFC 8785
    // bytes and their sha256. See `digest_is_stable_and_change_sensitive`.
    const CANONICAL_GOLDEN: &str = r#"{"branding":{"blurb":"blurb","display_name":"Clyso Enterprise Storage","footer":"footer"},"entries":[{"behavior_change":null,"blob_hash":"8d1c3243a35e2d54669fcc40dbb22ee8a6b9a4a9a35410cc816cf9175cd79c89","category":"fix","data_structure_change":null,"justification":{"internal":null,"kind":"engineering","public_summary":"Fixes a thing.","refs":["https://tracker.ceph.com/issues/1"]},"lifecycle":{"first_shipped_in":null,"status":"active"},"order":1,"patch_id":"pid-1","provenance":{"commits":["abc"],"prs":["https://github.com/ceph/ceph/pull/1"],"state":"merged-main","type":"upstream_pr"},"risk":{"blast":"availability","component":"rgw","conflict":"trivial","coverage":"partial"},"upgrade_notes":null,"visibility":"public"},{"behavior_change":null,"blob_hash":"33df0cf768d9f425aecca844921d344a6919929ed0563f4c83bcf0ae8118839d","category":"fix","data_structure_change":null,"justification":{"internal":null,"kind":"engineering","public_summary":"Fixes a thing.","refs":["https://tracker.ceph.com/issues/1"]},"lifecycle":{"first_shipped_in":null,"status":"active"},"order":2,"patch_id":"pid-2","provenance":{"description":"downstream-only","type":"other"},"risk":{"blast":"availability","component":"rgw","conflict":"trivial","coverage":"partial"},"upgrade_notes":null,"visibility":"public"}],"known_issues":[{"refs":[],"summary":"A known issue."}],"release":{"author":{"email":"rel@example.com","name":"Releaser"},"base_ref":"v18.2.0","channel":"ces","created":"2026-06-21T00:00:00+00:00","name":"ces-v18.2.0","namespace":"clyso-enterprise","product":"ceph"},"render":{"minijinja_version":"2.21.0","template_digest":"5cde0f1298f41f7d1c8b907a36992a7a513225a2615bd6e307bf1a9149b06b40"},"schema_version":1,"upgrade_notes":"Upgrade carefully."}"#;
    const DIGEST_GOLDEN: &str = "7c4bc17fff0e5bf0d64cc12ffe73592bec9c8817f36064514001ad4e729fe3e2";

    #[test]
    fn release_record_round_trips_through_json() {
        let manifest = sample_manifest();
        let digest = digest(&manifest).unwrap();
        let record = ReleaseRecord {
            manifest,
            digest,
            signature: ArmoredSignature("-----BEGIN PGP SIGNATURE-----…".to_owned()),
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: ReleaseRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn draft_round_trips_through_json() {
        let m = sample_manifest();
        let draft = Draft {
            release: m.release.clone(),
            entries: m.entries.clone(),
            known_issues: m.known_issues.clone(),
            upgrade_notes: m.upgrade_notes.clone(),
        };
        let json = serde_json::to_string(&draft).unwrap();
        let back: Draft = serde_json::from_str(&json).unwrap();
        assert_eq!(back, draft);
    }

    #[test]
    fn enums_serialize_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Visibility::Private).unwrap(),
            "\"private\""
        );
        assert_eq!(
            serde_json::to_string(&Blast::DataLoss).unwrap(),
            "\"data-loss\""
        );
        assert_eq!(serde_json::to_string(&Band::Medium).unwrap(), "\"medium\"");
        assert_eq!(
            serde_json::to_string(&PatchStatus::Superseded).unwrap(),
            "\"superseded\""
        );
    }
}
