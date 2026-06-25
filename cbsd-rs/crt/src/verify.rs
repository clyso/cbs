// crt — verify a sealed release (design §11, legs 0–2).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! `crt release verify <name>` runs the store-backed verification legs that are
//! applicable in M2 (design §11):
//!
//! - **Leg 0 — signature** (fail-fast): the `ReleaseRecord.signature` verifies
//!   over the canonical manifest with the published public key.
//! - **Leg 1 — schema:** the manifest's `schema_version` is the one this build
//!   supports.
//! - **Leg 2 — cross-reference:** the recomputed digest equals the stored one;
//!   every referenced patch blob exists; and each referenced `PatchMeta` agrees
//!   with the entry's denormalized `patch_id`.
//! - **Legs 3–4** (git anchoring, artifact faithfulness) need a materialized
//!   ref and are **reported as skipped** (M3/M4) — never silently passed.
//!
//! The core [`verify_release`] takes the public-key bytes, so it is fully
//! testable against an in-memory store with no network; [`load_public_key`] is
//! the thin edge shim that fetches them (a local path, or an http(s) URL).

use anyhow::{Context, Result, bail};
use crt_core::Sha256;
use crt_store::Store;

use crate::config::Config;

/// The verdict of a verification run. The three outcomes map to distinct
/// process exit codes in `main` (signature vs verify vs — for the `Err` arm —
/// operational), per design §11.
pub enum VerifyVerdict {
    /// Every applicable leg passed; the report lists each leg (incl. skipped).
    Pass(VerifyReport),
    /// Leg 0: the signature did not verify (or the public key was unusable).
    SignatureFailed(String),
    /// Leg 1 or 2: schema or cross-reference check failed.
    VerifyFailed(String),
}

/// Whether a leg ran or was skipped (not applicable in M2).
pub enum LegState {
    Passed,
    Skipped,
}

/// One leg's outcome, for reporting.
pub struct LegStatus {
    pub leg: &'static str,
    pub state: LegState,
    pub detail: String,
}

/// The per-leg report of a passing verification.
pub struct VerifyReport {
    pub legs: Vec<LegStatus>,
}

fn passed(leg: &'static str, detail: impl Into<String>) -> LegStatus {
    LegStatus {
        leg,
        state: LegState::Passed,
        detail: detail.into(),
    }
}

fn skipped(leg: &'static str, detail: impl Into<String>) -> LegStatus {
    LegStatus {
        leg,
        state: LegState::Skipped,
        detail: detail.into(),
    }
}

/// Verify the sealed release named `name` against `public_key_armored`. Returns
/// a [`VerifyVerdict`]; only operational problems (no such release, store
/// failure) surface as `Err`.
pub async fn verify_release(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
    public_key_armored: &str,
) -> Result<VerifyVerdict> {
    let key = cfg.resolve_release_key(name)?;
    let record = store
        .get_release(&key)
        .await
        .with_context(|| format!("no sealed release named {name:?}"))?;

    let canonical = crt_core::canonical_json(&record.manifest)?;

    // Leg 0 — signature (fail-fast). A bad signature or an unusable public key
    // both surface here; an invalid release must fail before anything else.
    if let Err(e) = crt_core::verify_manifest(&canonical, &record.signature, public_key_armored) {
        return Ok(VerifyVerdict::SignatureFailed(e.to_string()));
    }

    // Leg 1 — schema.
    if record.manifest.schema_version != crt_core::SCHEMA_VERSION {
        return Ok(VerifyVerdict::VerifyFailed(format!(
            "manifest schema_version {} is not the supported {}",
            record.manifest.schema_version,
            crt_core::SCHEMA_VERSION
        )));
    }

    // Leg 2 — cross-reference. The digest is the integrity anchor; it lives
    // outside the signed bytes, so a mismatch is caught here, not by leg 0.
    let recomputed = Sha256::of(&canonical);
    if recomputed != record.digest {
        return Ok(VerifyVerdict::VerifyFailed(format!(
            "recomputed digest {recomputed} != stored digest {}",
            record.digest
        )));
    }
    for entry in &record.manifest.entries {
        if !store.has_blob(&entry.blob_hash).await? {
            return Ok(VerifyVerdict::VerifyFailed(format!(
                "referenced patch blob {} is missing from the store",
                entry.blob_hash
            )));
        }
        // The referenced PatchMeta must exist and agree with the entry's
        // denormalized patch_id. A *missing* meta is a verify failure; a meta
        // that is present but unreadable surfaces as an operational error
        // (exit 1) rather than a verify failure (a narrow MVP simplification of
        // design §11 leg 1's "deserialize and validate").
        let meta = match store.get_meta(&entry.blob_hash).await {
            Ok(meta) => meta,
            Err(e) if e.is_not_found() => {
                return Ok(VerifyVerdict::VerifyFailed(format!(
                    "referenced patch meta for {} is missing",
                    entry.blob_hash
                )));
            }
            Err(e) => return Err(e.into()),
        };
        if meta.patch_id != entry.patch_id {
            return Ok(VerifyVerdict::VerifyFailed(format!(
                "entry {} records patch_id {:?} but the stored meta has {:?}",
                entry.blob_hash, entry.patch_id, meta.patch_id
            )));
        }
    }

    Ok(VerifyVerdict::Pass(VerifyReport {
        legs: vec![
            passed("0 signature", "detached OpenPGP signature valid"),
            passed(
                "1 schema",
                format!("schema_version {}", record.manifest.schema_version),
            ),
            passed(
                "2 cross-reference",
                format!(
                    "digest matches; {} referenced blob(s) present and consistent",
                    record.manifest.entries.len()
                ),
            ),
            skipped("3 git anchoring", "no materialized ref (M3/M4)"),
            skipped(
                "4 artifact faithfulness",
                "no materialized artifacts (M3/M4)",
            ),
        ],
    }))
}

/// Render a passing report for the operator. Skipped legs are shown explicitly
/// so a reader cannot mistake "not run" for "passed".
#[must_use]
pub fn render_report(report: &VerifyReport) -> String {
    let mut out = String::from("verify: OK\n");
    for leg in &report.legs {
        let tag = match leg.state {
            LegState::Passed => "pass",
            LegState::Skipped => "skip",
        };
        out.push_str(&format!("  [{tag}] leg {} — {}\n", leg.leg, leg.detail));
    }
    out
}

/// Fetch the armored public key from `source`: an `https://` URL (fetched with
/// `reqwest`) or, otherwise, a local file path (accepted for tests and for
/// operators who pin the key locally). Plaintext `http://` is **refused**: the
/// public key is the root of trust and fingerprint pinning is design-deferred,
/// so an unauthenticated transport is the one protection we still have.
pub async fn load_public_key(source: &str) -> Result<String> {
    if source.starts_with("http://") {
        bail!(
            "refusing to fetch the public key over plaintext http:// ({source:?}); \
             use https:// or a local file path"
        );
    }
    if source.starts_with("https://") {
        let resp = reqwest::get(source)
            .await
            .with_context(|| format!("fetching public key from {source}"))?
            .error_for_status()
            .with_context(|| format!("fetching public key from {source}"))?;
        resp.text()
            .await
            .with_context(|| format!("reading public key body from {source}"))
    } else {
        std::fs::read_to_string(source).with_context(|| format!("reading public key file {source}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crt_core::{
        Blast, Branding, Conflict, Coverage, Identity, Justification, JustificationKind, Lifecycle,
        Manifest, ManifestEntry, PatchMeta, PatchStatus, Provenance, ReleaseHeader, ReleaseKey,
        ReleaseRecord, RenderSpec, Risk, UpstreamPrState, Visibility, blob_hash,
    };
    use crt_store::ObjectBackedStore;

    use crate::config::{Channel, Namespace, StoreConfig};

    fn test_config() -> Config {
        let mut channels = BTreeMap::new();
        channels.insert(
            "ces".to_owned(),
            Channel {
                branding: Branding {
                    display_name: "Clyso Enterprise Storage".to_owned(),
                    blurb: "b".to_owned(),
                    footer: "f".to_owned(),
                },
            },
        );
        let mut namespaces = BTreeMap::new();
        namespaces.insert("clyso-enterprise".to_owned(), Namespace { channels });
        Config {
            component: "ceph".to_owned(),
            store: StoreConfig::Local(PathBuf::from("/tmp/store")),
            destination_repo: None,
            risk_components: vec![],
            namespaces,
            public_key_url: None,
        }
    }

    /// Generate a throwaway Ed25519 signing keypair (armored secret, public).
    fn test_keypair() -> (String, String) {
        use pgp::composed::{ArmorOptions, KeyType, SecretKeyParamsBuilder, SignedPublicKey};
        let mut params = SecretKeyParamsBuilder::default();
        params
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(true)
            .primary_user_id("CRT Verify Test <verify@example.com>".into())
            .passphrase(None);
        let secret_key = params
            .build()
            .expect("build key params")
            .generate(rand::thread_rng())
            .expect("generate key");
        let public_key = SignedPublicKey::from(secret_key.clone());
        (
            secret_key
                .to_armored_string(ArmorOptions::default())
                .expect("armor secret"),
            public_key
                .to_armored_string(ArmorOptions::default())
                .expect("armor public"),
        )
    }

    fn sample_entry(blob: Sha256, patch_id: &str) -> ManifestEntry {
        ManifestEntry {
            blob_hash: blob,
            patch_id: patch_id.to_owned(),
            order: 1,
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
                refs: vec![],
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
            provenance: Provenance::UpstreamPr {
                prs: vec![],
                commits: vec![],
                state: UpstreamPrState::MergedMain,
            },
        }
    }

    fn sample_manifest(schema_version: u32, entry: ManifestEntry) -> Manifest {
        Manifest {
            schema_version,
            release: ReleaseHeader {
                product: "ceph".to_owned(),
                namespace: "clyso-enterprise".to_owned(),
                channel: "ces".to_owned(),
                name: "ces-v18.2.0".to_owned(),
                base_ref: "v18.2.0".to_owned(),
                created: "2026-06-22T00:00:00+00:00".to_owned(),
                author: Identity {
                    name: "Releaser".to_owned(),
                    email: "rel@example.com".to_owned(),
                },
            },
            entries: vec![entry],
            known_issues: vec![],
            upgrade_notes: None,
            branding: Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "b".to_owned(),
                footer: "f".to_owned(),
            },
            render: RenderSpec {
                minijinja_version: "2.5.0".to_owned(),
                template_digest: blob_hash(b"template"),
            },
        }
    }

    fn signed_record(manifest: &Manifest, secret: &str) -> ReleaseRecord {
        let canonical = crt_core::canonical_json(manifest).unwrap();
        let digest = Sha256::of(&canonical);
        let signature =
            crt_core::sign_manifest(rand::thread_rng(), &canonical, secret, None).unwrap();
        ReleaseRecord {
            manifest: manifest.clone(),
            digest,
            signature,
        }
    }

    /// Store the patch blob + meta backing an entry, so cross-reference passes.
    async fn store_backing(store: &ObjectBackedStore, blob: Sha256, patch_id: &str) {
        store.put_blob(&blob, b"a patch").await.unwrap();
        store
            .put_meta(
                &blob,
                &PatchMeta {
                    blob_hash: blob,
                    patch_id: patch_id.to_owned(),
                    author: Identity {
                        name: "n".to_owned(),
                        email: "e@example.com".to_owned(),
                    },
                    authored: "2026-06-22T00:00:00+00:00".to_owned(),
                    subject: "s".to_owned(),
                    body: "b".to_owned(),
                    cherry_picked_from: vec![],
                    provenance: Provenance::UpstreamPr {
                        prs: vec![],
                        commits: vec![],
                        state: UpstreamPrState::MergedMain,
                    },
                    source_repo: "ceph/ceph".to_owned(),
                },
            )
            .await
            .unwrap();
    }

    fn key() -> ReleaseKey {
        ReleaseKey {
            namespace: "clyso-enterprise".to_owned(),
            channel: "ces".to_owned(),
            name: "ces-v18.2.0".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_sealed_release_verifies_and_reports_skipped_legs() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        let blob = blob_hash(b"a patch");
        store_backing(&store, blob, "pid-1").await;
        let record = signed_record(
            &sample_manifest(crt_core::SCHEMA_VERSION, sample_entry(blob, "pid-1")),
            &secret,
        );
        store.put_release(&key(), &record).await.unwrap();

        let verdict = verify_release(&store, &cfg, "ces-v18.2.0", &public)
            .await
            .unwrap();
        let report = match verdict {
            VerifyVerdict::Pass(r) => r,
            _ => panic!("expected Pass"),
        };
        let rendered = render_report(&report);
        assert!(rendered.contains("[pass] leg 0 signature"));
        assert!(rendered.contains("[pass] leg 2 cross-reference"));
        // Legs 3 and 4 are explicitly skipped, never silently passed.
        assert!(rendered.contains("[skip] leg 3 git anchoring"));
        assert!(rendered.contains("[skip] leg 4 artifact faithfulness"));
    }

    #[tokio::test]
    async fn a_tampered_manifest_fails_the_signature_leg() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        let blob = blob_hash(b"a patch");
        store_backing(&store, blob, "pid-1").await;

        // Sign the original manifest, then tamper it before storing — so the
        // signature is over different bytes than the stored manifest.
        let mut record = signed_record(
            &sample_manifest(crt_core::SCHEMA_VERSION, sample_entry(blob, "pid-1")),
            &secret,
        );
        record.manifest.upgrade_notes = Some("tampered after signing".to_owned());
        store.put_release(&key(), &record).await.unwrap();

        assert!(matches!(
            verify_release(&store, &cfg, "ces-v18.2.0", &public)
                .await
                .unwrap(),
            VerifyVerdict::SignatureFailed(_)
        ));
    }

    #[tokio::test]
    async fn a_wrong_digest_fails_cross_reference() {
        // The digest field is outside the signed bytes, so a forged digest
        // passes leg 0 but must fail leg 2 — the integrity anchor.
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        let blob = blob_hash(b"a patch");
        store_backing(&store, blob, "pid-1").await;

        let mut record = signed_record(
            &sample_manifest(crt_core::SCHEMA_VERSION, sample_entry(blob, "pid-1")),
            &secret,
        );
        record.digest = blob_hash(b"not the real digest");
        store.put_release(&key(), &record).await.unwrap();

        assert!(matches!(
            verify_release(&store, &cfg, "ces-v18.2.0", &public)
                .await
                .unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[tokio::test]
    async fn a_missing_blob_fails_cross_reference() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        let blob = blob_hash(b"a patch");
        // Sign correctly but never store the referenced blob/meta.
        let record = signed_record(
            &sample_manifest(crt_core::SCHEMA_VERSION, sample_entry(blob, "pid-1")),
            &secret,
        );
        store.put_release(&key(), &record).await.unwrap();

        assert!(matches!(
            verify_release(&store, &cfg, "ces-v18.2.0", &public)
                .await
                .unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[tokio::test]
    async fn a_patch_id_mismatch_fails_cross_reference() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        let blob = blob_hash(b"a patch");
        // The stored meta's patch_id disagrees with the entry's denormalized one.
        store_backing(&store, blob, "pid-STORED").await;
        let record = signed_record(
            &sample_manifest(crt_core::SCHEMA_VERSION, sample_entry(blob, "pid-ENTRY")),
            &secret,
        );
        store.put_release(&key(), &record).await.unwrap();

        assert!(matches!(
            verify_release(&store, &cfg, "ces-v18.2.0", &public)
                .await
                .unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[tokio::test]
    async fn an_unsupported_schema_version_fails() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        let blob = blob_hash(b"a patch");
        store_backing(&store, blob, "pid-1").await;
        let record = signed_record(&sample_manifest(999, sample_entry(blob, "pid-1")), &secret);
        store.put_release(&key(), &record).await.unwrap();

        assert!(matches!(
            verify_release(&store, &cfg, "ces-v18.2.0", &public)
                .await
                .unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[tokio::test]
    async fn a_freshly_authored_release_seals_then_verifies() {
        // The milestone seam: a release authored with `new`/`add`, sealed by
        // `seal_release`, must verify with `verify_release` end to end — not
        // just the two halves with independently-built fixtures.
        use crate::release::{EntryFields, add_entries, new_release, seal_release};
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();

        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            Identity {
                name: "Releaser".to_owned(),
                email: "rel@example.com".to_owned(),
            },
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();

        let blob = blob_hash(b"a patch");
        store_backing(&store, blob, "pid-1").await;
        let fields = EntryFields {
            visibility: Visibility::Public,
            category: "fix".to_owned(),
            component: "rgw".to_owned(),
            blast: Blast::Availability,
            conflict: Conflict::Trivial,
            coverage: Coverage::Partial,
            kind: JustificationKind::Engineering,
            refs: vec![],
            public_summary: "Fixes a thing.".to_owned(),
            internal: None,
            behavior_change: None,
            upgrade_notes: None,
        };
        add_entries(&store, &cfg, "ces-v18.2.0", &[blob.to_hex()], &fields)
            .await
            .unwrap();

        seal_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            &secret,
            None,
            rand::thread_rng(),
        )
        .await
        .unwrap();

        assert!(matches!(
            verify_release(&store, &cfg, "ces-v18.2.0", &public)
                .await
                .unwrap(),
            VerifyVerdict::Pass(_)
        ));
    }

    #[tokio::test]
    async fn load_public_key_refuses_plaintext_http() {
        // The root-of-trust key must never be fetched over an unauthenticated
        // transport; only https:// or a local path are accepted.
        assert!(
            load_public_key("http://example.com/crt-pubkey.asc")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn load_public_key_reads_a_local_file() {
        let (_, public) = test_keypair();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crt-pubkey.asc");
        std::fs::write(&path, &public).unwrap();
        let loaded = load_public_key(path.to_str().unwrap()).await.unwrap();
        assert_eq!(loaded, public);
    }

    /// Live URL fetch. Opt-in: set `CRT_TEST_PUBKEY_URL` and run with
    /// `cargo test -p crt -- --ignored`. The http(s) branch (reqwest + rustls)
    /// is otherwise unexercised in CI.
    #[tokio::test]
    #[ignore = "requires network; set CRT_TEST_PUBKEY_URL and run --ignored"]
    async fn load_public_key_fetches_a_url() {
        let url = std::env::var("CRT_TEST_PUBKEY_URL").expect("CRT_TEST_PUBKEY_URL");
        let key = load_public_key(&url).await.unwrap();
        assert!(key.contains("BEGIN PGP PUBLIC KEY"));
    }
}
