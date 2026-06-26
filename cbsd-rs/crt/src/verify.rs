// crt — verify a sealed release (design §11, legs 0–4).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! `crt release verify <name>` runs the design §11 verification legs. The
//! **sealed** legs 0–2 always run, over the store's signed `ReleaseRecord`:
//!
//! - **Leg 0 — signature** (fail-fast): the `ReleaseRecord.signature` verifies
//!   over the canonical manifest with the published public key.
//! - **Leg 1 — schema:** the manifest's `schema_version` is the one this build
//!   supports.
//! - **Leg 2 — cross-reference:** the recomputed digest equals the stored one;
//!   every referenced patch blob exists; and each referenced `PatchMeta` agrees
//!   with the entry's denormalized `patch_id`.
//!
//! When `repo` points at the destination repo **and** the release has been
//! materialized (its annotated tag exists), the **ref-conditional** legs also
//! run over the materialized git artifact — the bundle signature (leg 0), the
//! in-tree record's schema (leg 1) and its faithfulness to the sealed release
//! (leg 2), git anchoring (leg 3: `Crt-Patch` trailer + offset-invariant
//! `patch-id`), and artifact faithfulness (leg 4: SBOM/notes byte-compare).
//! Otherwise legs 3–4 are **reported skipped** — never silently passed.
//!
//! [`verify_tree`] is the offline subset (legs 0–2 over an extracted tree, no
//! store, no git) — the primary path for a tarball/ZIP/clone recipient, sharing
//! the in-tree checks via [`verify_tree_legs`]. The core [`verify_release`]
//! takes the public-key bytes, so it is testable against an in-memory store and
//! a local repo with no network; [`load_public_key`] is the thin edge shim that
//! fetches them (a local path, or an http(s) URL).

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use crt_core::{ArmoredSignature, Sha256};
use crt_store::Store;

use crate::config::Config;

/// The in-tree verification bundle directory (design §8).
pub(crate) const BUNDLE_DIR: &str = "000-RELEASE";
/// The signed record and its detached signature within [`BUNDLE_DIR`].
pub(crate) const RECORD_JSON: &str = "record.json";
pub(crate) const RECORD_SIG: &str = "record.json.asc";

/// The verdict of a verification run. Each outcome carries the per-leg
/// [`VerifyReport`] (so a failure names the failing leg, design §11 F9), and the
/// three outcomes map to distinct process exit codes in `main` (signature vs
/// verify vs — for the `Err` arm — operational).
pub enum VerifyVerdict {
    /// Every applicable leg passed; the report lists each leg (incl. skipped).
    Pass(VerifyReport),
    /// A signature leg (leg 0, sealed or bundle) did not verify.
    SignatureFailed(VerifyReport),
    /// A schema / cross-reference / anchoring / faithfulness leg (1–4) failed.
    VerifyFailed(VerifyReport),
}

/// Whether a leg passed, was skipped (not applicable — e.g. no materialized
/// ref), or failed.
pub enum LegState {
    Passed,
    Skipped,
    Failed,
}

/// One leg's outcome, for reporting.
pub struct LegStatus {
    pub leg: &'static str,
    pub state: LegState,
    pub detail: String,
}

/// The per-leg report of a verification run (passing or failing).
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

fn failed(leg: &'static str, detail: impl Into<String>) -> LegStatus {
    LegStatus {
        leg,
        state: LegState::Failed,
        detail: detail.into(),
    }
}

/// The verdict kind a leg group resolved to, before it is paired with the
/// accumulated report into a [`VerifyVerdict`].
enum VerdictKind {
    Pass,
    SignatureFailed,
    VerifyFailed,
}

/// Everything the blocking ref-verification needs from the sealed release —
/// pre-computed by the async caller so the blocking section touches no store.
struct SealedRefInputs {
    /// The sealed manifest digest the in-tree record must back-reference.
    manifest_digest: Sha256,
    base_ref: String,
    /// `blob_hash` hex → (`order`, `patch_id`) of the sealed entries.
    entries: BTreeMap<String, (u32, String)>,
    /// The leg-4 re-derivations from the **sealed** manifest, to byte-compare.
    sbom_expected: Vec<u8>,
    notes_expected: Vec<u8>,
}

/// Verify the sealed release `name` against `public_key_armored`. The sealed
/// legs 0–2 (signature, schema, cross-reference) always run. When `repo` points
/// at the destination repo **and** the release has been materialized (its
/// annotated tag exists), the ref-conditional legs additionally run over the
/// materialized git artifact: the bundle signature (leg 0), the in-tree record's
/// schema (leg 1) and its faithfulness to the sealed release (leg 2), git
/// anchoring (leg 3), and artifact faithfulness (leg 4) — otherwise legs 3–4 are
/// reported `skipped`.
///
/// Each [`VerifyVerdict`] carries the per-leg report; only operational problems
/// (no such release, store/git failure) surface as `Err`.
pub async fn verify_release(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
    public_key_armored: &str,
    repo: Option<&Path>,
) -> Result<VerifyVerdict> {
    let key = cfg.resolve_release_key(name)?;
    let record = store
        .get_release(&key)
        .await
        .with_context(|| format!("no sealed release named {name:?}"))?;

    let canonical = crt_core::canonical_json(&record.manifest)?;
    let mut legs: Vec<LegStatus> = Vec::new();
    const SEAL_0: &str = "0 signature (sealed manifest)";
    const SEAL_1: &str = "1 schema (sealed manifest)";
    const SEAL_2: &str = "2 cross-reference (sealed manifest)";

    // Leg 0 (sealed) — signature, fail-fast. A bad signature or an unusable
    // public key both surface here; an invalid release fails before anything.
    if let Err(e) = crt_core::verify_manifest(&canonical, &record.signature, public_key_armored) {
        legs.push(failed(SEAL_0, e.to_string()));
        return Ok(VerifyVerdict::SignatureFailed(VerifyReport { legs }));
    }
    legs.push(passed(SEAL_0, "detached OpenPGP signature valid"));

    // Leg 1 (sealed) — schema.
    if record.manifest.schema_version != crt_core::SCHEMA_VERSION {
        legs.push(failed(
            SEAL_1,
            format!(
                "manifest schema_version {} is not the supported {}",
                record.manifest.schema_version,
                crt_core::SCHEMA_VERSION
            ),
        ));
        return Ok(VerifyVerdict::VerifyFailed(VerifyReport { legs }));
    }
    legs.push(passed(
        SEAL_1,
        format!("schema_version {}", record.manifest.schema_version),
    ));

    // Leg 2 (sealed) — cross-reference. The digest is the integrity anchor; it
    // lives outside the signed bytes, so a mismatch is caught here, not leg 0.
    let recomputed = Sha256::of(&canonical);
    if recomputed != record.digest {
        legs.push(failed(
            SEAL_2,
            format!(
                "recomputed digest {recomputed} != stored digest {}",
                record.digest
            ),
        ));
        return Ok(VerifyVerdict::VerifyFailed(VerifyReport { legs }));
    }
    for entry in &record.manifest.entries {
        if !store.has_blob(&entry.blob_hash).await? {
            legs.push(failed(
                SEAL_2,
                format!(
                    "referenced patch blob {} is missing from the store",
                    entry.blob_hash
                ),
            ));
            return Ok(VerifyVerdict::VerifyFailed(VerifyReport { legs }));
        }
        // The referenced PatchMeta must exist and agree with the entry's
        // denormalized patch_id. A *missing* meta is a verify failure; a meta
        // present but unreadable surfaces as an operational error (exit 1)
        // rather than a verify failure (the MVP simplification of §11 leg 1).
        let meta = match store.get_meta(&entry.blob_hash).await {
            Ok(meta) => meta,
            Err(e) if e.is_not_found() => {
                legs.push(failed(
                    SEAL_2,
                    format!("referenced patch meta for {} is missing", entry.blob_hash),
                ));
                return Ok(VerifyVerdict::VerifyFailed(VerifyReport { legs }));
            }
            Err(e) => return Err(e.into()),
        };
        if meta.patch_id != entry.patch_id {
            legs.push(failed(
                SEAL_2,
                format!(
                    "entry {} records patch_id {:?} but the stored meta has {:?}",
                    entry.blob_hash, entry.patch_id, meta.patch_id
                ),
            ));
            return Ok(VerifyVerdict::VerifyFailed(VerifyReport { legs }));
        }
    }
    legs.push(passed(
        SEAL_2,
        format!(
            "digest matches; {} referenced blob(s) present and consistent",
            record.manifest.entries.len()
        ),
    ));

    // Ref-conditional legs. Without a destination repo there is nothing to
    // anchor against, so legs 3–4 are reported skipped (never silently passed).
    let Some(repo_path) = repo else {
        legs.push(skipped(
            "3 git anchoring",
            "no destination repo (pass --repo)",
        ));
        legs.push(skipped(
            "4 artifact faithfulness",
            "no destination repo (pass --repo)",
        ));
        return Ok(VerifyVerdict::Pass(VerifyReport { legs }));
    };

    // Re-derive the deterministic artifacts from the *sealed* manifest for leg 4
    // (version-gated render — a minijinja mismatch is an operational error, as
    // in `release notes`). The byte-compare happens in the blocking section.
    let sbom_expected = crt_core::build_sbom(&record.manifest)?.into_bytes();
    let notes_expected = crate::release::render_notes_for(store, &record)
        .await?
        .into_bytes();

    let sealed = SealedRefInputs {
        manifest_digest: record.digest,
        base_ref: record.manifest.release.base_ref.clone(),
        entries: record
            .manifest
            .entries
            .iter()
            .map(|e| (e.blob_hash.to_hex(), (e.order, e.patch_id.clone())))
            .collect(),
        sbom_expected,
        notes_expected,
    };
    let repo_path = repo_path.to_path_buf();
    let tag = name.to_owned();
    let public_key = public_key_armored.to_owned();

    // Subprocess git + a tree checkout + a tree walk are blocking; offload them.
    let (ref_legs, kind) = tokio::task::spawn_blocking(move || {
        verify_ref_legs(&repo_path, &tag, &public_key, &sealed)
    })
    .await
    .context("the ref-verification task panicked")??;
    legs.extend(ref_legs);
    Ok(match kind {
        VerdictKind::Pass => VerifyVerdict::Pass(VerifyReport { legs }),
        VerdictKind::SignatureFailed => VerifyVerdict::SignatureFailed(VerifyReport { legs }),
        VerdictKind::VerifyFailed => VerifyVerdict::VerifyFailed(VerifyReport { legs }),
    })
}

/// Blocking: when the materialized tag exists, check the tree out and run the
/// ref-conditional legs (0' bundle signature, 1'/2' in-tree record, 3 git
/// anchoring, 4 artifact faithfulness); otherwise return the two `skipped` legs.
/// The checkout uses `core.autocrlf=false` so its bytes equal the materialize
/// worktree's (and a recipient's plain extraction) — **not** `git archive`,
/// which would re-apply `.gitattributes` (design §8/§14).
fn verify_ref_legs(
    repo: &Path,
    tag: &str,
    public_key: &str,
    sealed: &SealedRefInputs,
) -> Result<(Vec<LegStatus>, VerdictKind)> {
    if !crate::git::tag_exists(repo, tag)? {
        return Ok((
            vec![
                skipped("3 git anchoring", "release not materialized (no tag)"),
                skipped(
                    "4 artifact faithfulness",
                    "release not materialized (no tag)",
                ),
            ],
            VerdictKind::Pass,
        ));
    }
    let wt = crate::git::checkout_detached(repo, tag)?;
    let result = verify_ref_legs_in_tree(wt.path(), repo, tag, public_key, sealed);
    // Read-only audit: the verdict is already captured, so a teardown failure
    // must not mask it — warn and leave the scratch worktree.
    if let Err(e) = wt.remove() {
        eprintln!("warning: removing the verification worktree failed: {e:#}");
    }
    result
}

/// The ref-conditional legs over the checked-out materialized `tree`.
fn verify_ref_legs_in_tree(
    tree: &Path,
    repo: &Path,
    tag: &str,
    public_key: &str,
    sealed: &SealedRefInputs,
) -> Result<(Vec<LegStatus>, VerdictKind)> {
    let mut legs: Vec<LegStatus> = Vec::new();

    // Legs 0–2 over the in-tree record + extracted tree (shared with
    // `verify --tree`): bundle signature, record schema, source + bundle digests.
    let record = match verify_tree_legs(tree, public_key, &mut legs)? {
        TreeOutcome::Ok(record) => *record,
        TreeOutcome::SignatureFailed => return Ok((legs, VerdictKind::SignatureFailed)),
        TreeOutcome::VerifyFailed => return Ok((legs, VerdictKind::VerifyFailed)),
    };

    // Leg 2 (extend) — the in-tree record is a faithful projection of the sealed
    // release: it back-references the sealed digest, and its BOM equals the
    // sealed entries on (order, blob_hash, patch_id) and count. The BOM's
    // patch_id was copied from the sealed entry at materialize time, so this
    // catches a tampered-but-validly-signed record.json, not a value mismatch.
    const IN_2: &str = "2 cross-reference (in-tree record)";
    if record.s3_manifest_digest != sealed.manifest_digest {
        legs.push(failed(
            IN_2,
            format!(
                "in-tree s3_manifest_digest {} != sealed digest {}",
                record.s3_manifest_digest, sealed.manifest_digest
            ),
        ));
        return Ok((legs, VerdictKind::VerifyFailed));
    }
    let bom: BTreeMap<String, (u32, String)> = record
        .patches
        .iter()
        .map(|p| (p.blob_hash.to_hex(), (p.order, p.patch_id.clone())))
        .collect();
    if bom.len() != record.patches.len() || bom != sealed.entries {
        legs.push(failed(
            IN_2,
            "in-tree patch BOM is not a faithful projection of the sealed entries",
        ));
        return Ok((legs, VerdictKind::VerifyFailed));
    }
    legs.push(passed(
        IN_2,
        format!(
            "back-reference + {}-entry BOM match the sealed release",
            record.patches.len()
        ),
    ));

    // Leg 3 — git anchoring. Each patch commit's Crt-Patch trailer names its
    // blob, and the patch_id recomputed from the commit's diff (offset-invariant)
    // equals the sealed entry's.
    match anchor_history(repo, tag, &sealed.base_ref, &sealed.entries)? {
        Ok(n) => legs.push(passed(
            "3 git anchoring",
            format!("{n} commit(s) anchor to the sealed entries via Crt-Patch + patch-id"),
        )),
        Err(reason) => {
            legs.push(failed("3 git anchoring", reason));
            return Ok((legs, VerdictKind::VerifyFailed));
        }
    }

    // Leg 4 — artifact faithfulness: the committed SBOM + notes byte-match a
    // re-derivation from the sealed manifest.
    let bundle = tree.join(BUNDLE_DIR);
    const IN_4: &str = "4 artifact faithfulness";
    if let Err(reason) = byte_match(
        &bundle.join("sbom.cdx.json"),
        &sealed.sbom_expected,
        "sbom.cdx.json",
    ) {
        legs.push(failed(IN_4, reason));
        return Ok((legs, VerdictKind::VerifyFailed));
    }
    if let Err(reason) = byte_match(
        &bundle.join("RELEASE-NOTES.md"),
        &sealed.notes_expected,
        "RELEASE-NOTES.md",
    ) {
        legs.push(failed(IN_4, reason));
        return Ok((legs, VerdictKind::VerifyFailed));
    }
    legs.push(passed(
        IN_4,
        "sbom.cdx.json + RELEASE-NOTES.md byte-match a re-derivation from the sealed manifest",
    ));

    Ok((legs, VerdictKind::Pass))
}

/// Leg 3: walk `base_ref..tag^` (the patch commits, oldest first) and confirm
/// each commit's `Crt-Patch` trailer names a sealed entry whose `patch_id`
/// equals the one recomputed from the **commit's diff** via
/// `git patch-id --stable` (offset-invariant — it verifies what `git am`
/// actually landed, not a re-derivation of the import-time id). The outer
/// `Result` is operational (a git failure); the inner names the leg verdict
/// (`Ok(count)` or `Err(reason)`).
fn anchor_history(
    repo: &Path,
    tag: &str,
    base_ref: &str,
    entries: &BTreeMap<String, (u32, String)>,
) -> Result<std::result::Result<usize, String>> {
    let range = format!("{base_ref}..{tag}^");
    let log = crate::git::git(repo, &["log", "--reverse", "--format=%H", &range])
        .with_context(|| format!("walking the materialized history {range}"))?;
    let commits: Vec<&str> = log.split_whitespace().collect();
    for sha in &commits {
        let trailer = crate::git::git(
            repo,
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Crt-Patch,valueonly)",
                sha,
            ],
        )?;
        let Some(blob_hex) = trailer.trim().strip_prefix("sha256:").map(str::to_owned) else {
            return Ok(Err(format!("commit {sha} has no Crt-Patch trailer")));
        };
        let Some((_, patch_id)) = entries.get(&blob_hex) else {
            return Ok(Err(format!(
                "commit {sha} Crt-Patch {blob_hex} matches no sealed entry"
            )));
        };
        let patch = crate::git::git_bytes(repo, &["format-patch", "-1", "--stdout", sha])?;
        let out = crate::git::git_with_stdin(repo, &["patch-id", "--stable"], &patch)?;
        let recomputed = out.split_whitespace().next().unwrap_or("");
        if recomputed != patch_id {
            return Ok(Err(format!(
                "commit {sha}: recomputed patch_id {recomputed} != sealed entry patch_id {patch_id}"
            )));
        }
    }
    Ok(Ok(commits.len()))
}

/// Read `path` and compare its bytes to `expected`; `Err(reason)` on any
/// difference or read error (leg-4 helper).
fn byte_match(path: &Path, expected: &[u8], label: &str) -> std::result::Result<(), String> {
    match std::fs::read(path) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(_) => Err(format!(
            "{BUNDLE_DIR}/{label} does not byte-match a re-derivation from the sealed manifest"
        )),
        Err(e) => Err(format!("cannot read {BUNDLE_DIR}/{label}: {e}")),
    }
}

/// Render the per-leg report for the operator. Skipped legs are shown
/// explicitly so a reader cannot mistake "not run" for "passed", and a failing
/// leg is tagged `FAIL` (design §11 F9). The caller prints the overall verdict
/// header; this renders only the legs.
#[must_use]
pub fn render_report(report: &VerifyReport) -> String {
    let mut out = String::new();
    for leg in &report.legs {
        let tag = match leg.state {
            LegState::Passed => "pass",
            LegState::Skipped => "skip",
            LegState::Failed => "FAIL",
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

/// The outcome of the offline in-tree legs: the validated record, or which leg
/// group failed (the legs are already pushed onto the caller's report).
enum TreeOutcome {
    Ok(Box<crt_core::MaterializationRecord>),
    SignatureFailed,
    VerifyFailed,
}

/// `crt verify --tree <dir>`: offline / detached-tree verification of an
/// extracted release tree (design §10/§11) — no store, no git. The primary
/// trust path for a tarball/ZIP/clone recipient; it runs legs 0–2 (and never
/// leg 4, §11) via the shared [`verify_tree_legs`].
///
/// Blocking (walks the tree); a caller under an async runtime must offload it
/// (e.g. `tokio::task::spawn_blocking`).
pub fn verify_tree(tree_dir: &Path, public_key_armored: &str) -> Result<VerifyVerdict> {
    let mut legs = Vec::new();
    let kind = match verify_tree_legs(tree_dir, public_key_armored, &mut legs)? {
        TreeOutcome::Ok(_) => VerdictKind::Pass,
        TreeOutcome::SignatureFailed => VerdictKind::SignatureFailed,
        TreeOutcome::VerifyFailed => VerdictKind::VerifyFailed,
    };
    Ok(match kind {
        VerdictKind::Pass => VerifyVerdict::Pass(VerifyReport { legs }),
        VerdictKind::SignatureFailed => VerifyVerdict::SignatureFailed(VerifyReport { legs }),
        VerdictKind::VerifyFailed => VerifyVerdict::VerifyFailed(VerifyReport { legs }),
    })
}

/// The offline in-tree legs over an extracted tree, pushing each outcome onto
/// `legs` (design §10/§11): verify `000-RELEASE/record.json.asc` over the **raw**
/// `record.json` bytes (leg 0), deserialize + schema-check the record (leg 1),
/// then recompute `source_tree_digest` over the extracted source and **every**
/// `bundle_digests` entry, requiring the bundle file set to be exactly
/// `bundle_digests` (leg 2). Shared by `verify --tree` and the ref-conditional
/// `release verify`. Returns the validated record on success.
fn verify_tree_legs(
    tree_dir: &Path,
    public_key_armored: &str,
    legs: &mut Vec<LegStatus>,
) -> Result<TreeOutcome> {
    const BUNDLE_0: &str = "0 signature (000-RELEASE bundle)";
    const BUNDLE_1: &str = "1 schema (in-tree record)";
    const BUNDLE_2: &str = "2 source + bundle digests";
    let bundle = tree_dir.join(BUNDLE_DIR);

    let record_bytes = match std::fs::read(bundle.join(RECORD_JSON)) {
        Ok(b) => b,
        Err(e) => {
            legs.push(failed(
                BUNDLE_0,
                format!("cannot read {BUNDLE_DIR}/{RECORD_JSON}: {e}"),
            ));
            return Ok(TreeOutcome::SignatureFailed);
        }
    };
    let signature = match std::fs::read_to_string(bundle.join(RECORD_SIG)) {
        Ok(s) => ArmoredSignature(s),
        Err(e) => {
            legs.push(failed(
                BUNDLE_0,
                format!("cannot read {BUNDLE_DIR}/{RECORD_SIG}: {e}"),
            ));
            return Ok(TreeOutcome::SignatureFailed);
        }
    };

    // Leg 0 — signature over the EXACT on-disk record bytes. The `.asc` signs
    // the file verbatim; there is no canonicalization or separate digest field,
    // so re-serializing here would risk a byte drift that silently fails.
    if let Err(e) = crt_core::verify_manifest(&record_bytes, &signature, public_key_armored) {
        legs.push(failed(BUNDLE_0, e.to_string()));
        return Ok(TreeOutcome::SignatureFailed);
    }
    legs.push(passed(
        BUNDLE_0,
        "detached OpenPGP signature over record.json valid",
    ));

    // Leg 1 — schema. Deserialize the SAME bytes the signature just covered.
    let record: crt_core::MaterializationRecord = match serde_json::from_slice(&record_bytes) {
        Ok(r) => r,
        Err(e) => {
            legs.push(failed(
                BUNDLE_1,
                format!("{BUNDLE_DIR}/{RECORD_JSON} does not deserialize: {e}"),
            ));
            return Ok(TreeOutcome::VerifyFailed);
        }
    };
    if record.schema_version != crt_core::MATERIALIZATION_RECORD_VERSION {
        legs.push(failed(
            BUNDLE_1,
            format!(
                "record schema_version {} is not the supported {}",
                record.schema_version,
                crt_core::MATERIALIZATION_RECORD_VERSION
            ),
        ));
        return Ok(TreeOutcome::VerifyFailed);
    }
    legs.push(passed(
        BUNDLE_1,
        format!("record schema_version {}", record.schema_version),
    ));

    // Leg 2 — recompute the source-tree digest over the extracted files
    // (excluding the bundle + git dirs) and every bundle digest.
    let recomputed = crt_core::source_tree_digest(&walk_source_tree(tree_dir)?);
    if recomputed != record.source_tree_digest {
        legs.push(failed(
            BUNDLE_2,
            format!(
                "recomputed source_tree_digest {recomputed} != record {}",
                record.source_tree_digest
            ),
        ));
        return Ok(TreeOutcome::VerifyFailed);
    }
    if let Some(reason) = verify_bundle_digests(&bundle, &record.bundle_digests)? {
        legs.push(failed(BUNDLE_2, reason));
        return Ok(TreeOutcome::VerifyFailed);
    }
    legs.push(passed(
        BUNDLE_2,
        format!(
            "source_tree_digest matches; {} bundle file(s) match and are exhaustive",
            record.bundle_digests.len()
        ),
    ));

    Ok(TreeOutcome::Ok(Box::new(record)))
}

/// Compare every flat `000-RELEASE/` file (except the record and its signature)
/// against `expected`, and require the two sets to match exactly — so any file
/// added to the bundle is forced under the signed record (no unsigned bundle
/// content). Returns `Some(reason)` on the first mismatch, `None` when sound.
fn verify_bundle_digests(
    bundle: &Path,
    expected: &BTreeMap<String, Sha256>,
) -> Result<Option<String>> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in
        std::fs::read_dir(bundle).with_context(|| format!("reading {}", bundle.display()))?
    {
        let entry = entry.with_context(|| format!("reading an entry in {}", bundle.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat {}", entry.path().display()))?;
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => {
                return Ok(Some(format!(
                    "a {BUNDLE_DIR}/ file name is not valid UTF-8"
                )));
            }
        };
        if !file_type.is_file() {
            return Ok(Some(format!(
                "unexpected non-file {name:?} in {BUNDLE_DIR}/ (the bundle is flat files only)"
            )));
        }
        if name == RECORD_JSON || name == RECORD_SIG {
            continue;
        }
        let content = std::fs::read(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        let actual = Sha256::of(&content);
        match expected.get(&name) {
            None => {
                return Ok(Some(format!(
                    "bundle file {name:?} is not covered by bundle_digests (unsigned content)"
                )));
            }
            Some(h) if *h != actual => {
                return Ok(Some(format!(
                    "bundle file {name:?} digest {actual} != record {h}"
                )));
            }
            Some(_) => {
                seen.insert(name);
            }
        }
    }
    // Exhaustiveness the other way: bundle_digests must not name a file absent
    // from the bundle.
    for name in expected.keys() {
        if !seen.contains(name) {
            return Ok(Some(format!(
                "bundle_digests names {name:?} but it is absent from {BUNDLE_DIR}/"
            )));
        }
    }
    Ok(None)
}

/// Walk the materialized source tree rooted at `root`, returning each
/// non-excluded file's repo-relative slash path mapped to a domain-separated
/// content hash (design §8/§14, plan M4 decision 3). Regular files hash their
/// bytes tagged `f\0`; symlinks hash their target tagged `l\0` (recorded by
/// target so a ZIP/tar recipient — which may not follow links — agrees, and so
/// a file↔symlink swap cannot collide). Mode is ignored (content-only), so an
/// executable bit dropped by a ZIP extraction does not move the digest. The
/// top-level `.git` and `000-RELEASE/` are excluded (§8) — `.git` whether it is
/// a directory (a clone) or a file (a linked worktree's gitlink).
///
/// The digest hashes raw on-disk bytes, so it assumes a **faithful** extraction:
/// a plain archive of the worktree, or a clone with content filters off (the
/// canonical distribution per decision 3). A clone under `core.autocrlf=true`
/// would rewrite source-file line endings and recompute a different digest —
/// out of scope for the MVP, which distributes archives.
pub fn walk_source_tree(root: &Path) -> Result<BTreeMap<String, Sha256>> {
    let mut files = BTreeMap::new();
    walk_into(root, root, &mut files)?;
    Ok(files)
}

fn walk_into(root: &Path, dir: &Path, files: &mut BTreeMap<String, Sha256>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        let path = entry.path();
        let rel = rel_slash_path(root, &path)?;
        // Exclude the top-level git and bundle entries (§8) *before* inspecting
        // the node type: in a linked git worktree `.git` is a **file** (a
        // `gitdir:` pointer), not a directory, and its bytes differ per checkout
        // — hashing it would make the digest non-portable. The check is
        // top-level (`rel == ".git"`), so a nested file literally named `.git`
        // is unaffected.
        if rel == ".git" || rel == BUNDLE_DIR {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat {}", path.display()))?;
        if file_type.is_symlink() {
            let target = std::fs::read_link(&path)
                .with_context(|| format!("reading symlink {}", path.display()))?;
            let mut tagged = b"l\0".to_vec();
            tagged.extend_from_slice(target.as_os_str().as_bytes());
            files.insert(rel, Sha256::of(&tagged));
        } else if file_type.is_dir() {
            walk_into(root, &path, files)?;
        } else if file_type.is_file() {
            let content =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let mut tagged = b"f\0".to_vec();
            tagged.extend_from_slice(&content);
            files.insert(rel, Sha256::of(&tagged));
        }
        // Other node types (fifo, socket, device) don't occur in a source tree.
    }
    Ok(())
}

/// `path` relative to `root`, as a canonical `/`-joined UTF-8 string.
fn rel_slash_path(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    parts
        .map(|p| p.join("/"))
        .with_context(|| format!("path {} has a non-UTF-8 component", rel.display()))
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

        let verdict = verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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
            verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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
            verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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
            verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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
            verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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
            verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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
            verify_release(&store, &cfg, "ces-v18.2.0", &public, None)
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

    /// Build a materialized tree under `dir`: a few source files (plus an
    /// excluded `.git/`) and a signed, self-consistent `000-RELEASE/` bundle.
    /// Returns the armored public key that verifies it.
    fn build_signed_tree(dir: &Path) -> String {
        let (secret, public) = test_keypair();

        // Source files + an excluded .git/ (000-RELEASE/ is created below, after
        // the source digest is taken).
        std::fs::write(dir.join("src.txt"), "source\n").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/nested.txt"), "nested\n").unwrap();
        std::fs::create_dir(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/config"), "ignored").unwrap();
        let source_digest = crt_core::source_tree_digest(&walk_source_tree(dir).unwrap());

        // The bundle: the public-facing files, then the record + signature.
        let bundle = dir.join("000-RELEASE");
        std::fs::create_dir(&bundle).unwrap();
        let mut bundle_digests = BTreeMap::new();
        for (name, content) in [
            ("sbom.cdx.json", "{\"bomFormat\":\"CycloneDX\"}\n"),
            ("RELEASE-NOTES.md", "# Notes\n"),
            ("provenance.json", "{\"patches\":[]}\n"),
            ("README.md", "# 000-RELEASE\n"),
            // Inside `000-RELEASE/`, a slash-free pattern matches every file in
            // that directory; `000-RELEASE/* -text` would be resolved relative
            // to the file's own dir (`000-RELEASE/000-RELEASE/*`) and match
            // nothing. This mirrors what 4.3 actually writes.
            (".gitattributes", "* -text\n"),
        ] {
            std::fs::write(bundle.join(name), content).unwrap();
            bundle_digests.insert(name.to_owned(), Sha256::of(content.as_bytes()));
        }

        let record = crt_core::MaterializationRecord {
            schema_version: crt_core::MATERIALIZATION_RECORD_VERSION,
            s3_manifest_digest: blob_hash(b"sealed-manifest"),
            base_ref: "v18.2.0".to_owned(),
            created: "2026-06-25T00:00:00+00:00".to_owned(),
            render: RenderSpec {
                minijinja_version: "2.21.0".to_owned(),
                template_digest: blob_hash(b"template"),
            },
            source_tree_digest: source_digest,
            bundle_digests,
            patches: vec![crt_core::MaterializedPatch {
                order: 1,
                blob_hash: blob_hash(b"blob"),
                patch_id: "pid-1".to_owned(),
                git_commit: "abc123".to_owned(),
            }],
        };
        // Serialize ONCE; sign those exact bytes; write both. The verifier reads
        // the record bytes verbatim (no canonicalization), so producer and
        // verifier must agree on the literal bytes.
        let record_bytes = serde_json::to_vec_pretty(&record).unwrap();
        let sig =
            crt_core::sign_manifest(rand::thread_rng(), &record_bytes, &secret, None).unwrap();
        std::fs::write(bundle.join("record.json"), &record_bytes).unwrap();
        std::fs::write(bundle.join("record.json.asc"), &sig.0).unwrap();
        public
    }

    #[test]
    fn verify_tree_passes_on_a_well_formed_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let public = build_signed_tree(dir.path());
        assert!(matches!(
            verify_tree(dir.path(), &public).unwrap(),
            VerifyVerdict::Pass(_)
        ));
    }

    #[test]
    fn verify_tree_fails_on_a_mutated_source_file() {
        let dir = tempfile::tempdir().unwrap();
        let public = build_signed_tree(dir.path());
        std::fs::write(dir.path().join("src.txt"), "TAMPERED\n").unwrap();
        assert!(matches!(
            verify_tree(dir.path(), &public).unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[test]
    fn verify_tree_fails_on_a_mutated_bundle_file() {
        let dir = tempfile::tempdir().unwrap();
        let public = build_signed_tree(dir.path());
        std::fs::write(
            dir.path().join("000-RELEASE/sbom.cdx.json"),
            "{\"tampered\":true}\n",
        )
        .unwrap();
        assert!(matches!(
            verify_tree(dir.path(), &public).unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[test]
    fn verify_tree_fails_on_a_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let _ = build_signed_tree(dir.path());
        let (_, other_public) = test_keypair();
        assert!(matches!(
            verify_tree(dir.path(), &other_public).unwrap(),
            VerifyVerdict::SignatureFailed(_)
        ));
    }

    #[test]
    fn verify_tree_fails_on_a_stripped_signature() {
        let dir = tempfile::tempdir().unwrap();
        let public = build_signed_tree(dir.path());
        std::fs::remove_file(dir.path().join("000-RELEASE/record.json.asc")).unwrap();
        assert!(matches!(
            verify_tree(dir.path(), &public).unwrap(),
            VerifyVerdict::SignatureFailed(_)
        ));
    }

    #[test]
    fn verify_tree_fails_on_an_unsigned_bundle_file() {
        // Exhaustiveness (v2 plan-review N2): a 000-RELEASE/ file absent from
        // bundle_digests must fail — nothing in the bundle may escape the record.
        let dir = tempfile::tempdir().unwrap();
        let public = build_signed_tree(dir.path());
        std::fs::write(dir.path().join("000-RELEASE/sneaked.txt"), "unsigned").unwrap();
        assert!(matches!(
            verify_tree(dir.path(), &public).unwrap(),
            VerifyVerdict::VerifyFailed(_)
        ));
    }

    #[test]
    fn source_tree_digest_survives_a_tar_roundtrip_with_attrs_exec_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        let w = work.path();
        std::fs::write(w.join("src.txt"), "hello\n").unwrap();
        std::fs::write(w.join(".gitattributes"), "* text=auto\n").unwrap();
        std::fs::write(w.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::os::unix::fs::symlink("src.txt", w.join("link.txt")).unwrap();
        // Excluded dirs with content, to prove they never enter the digest.
        std::fs::create_dir(w.join(".git")).unwrap();
        std::fs::write(w.join(".git/HEAD"), "ref: x").unwrap();
        std::fs::create_dir(w.join("000-RELEASE")).unwrap();
        std::fs::write(w.join("000-RELEASE/record.json"), "{}").unwrap();

        let files = walk_source_tree(w).unwrap();
        assert!(files.contains_key("src.txt"));
        assert!(files.contains_key(".gitattributes"));
        assert!(files.contains_key("run.sh"));
        assert!(files.contains_key("link.txt"));
        // The excluded *directories* contribute no entries — but `.gitattributes`
        // is a legitimate source file and must remain (hence the trailing slash).
        assert!(
            !files
                .keys()
                .any(|k| k.starts_with(".git/") || k.starts_with("000-RELEASE/")),
            "excluded dirs must not appear in the digest input"
        );
        let dig_work = crt_core::source_tree_digest(&files);

        // An executable-bit flip is invisible (content-only / ignore mode).
        let mut perm = std::fs::metadata(w.join("run.sh")).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(w.join("run.sh"), perm).unwrap();
        assert_eq!(
            dig_work,
            crt_core::source_tree_digest(&walk_source_tree(w).unwrap()),
            "an exec-bit flip must not move the digest"
        );

        // A plain tar of the worktree (minus .git), extracted elsewhere, yields
        // the identical digest — the worktree ≡ tarball-recipient contract.
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("tree.tar");
        assert!(
            std::process::Command::new("tar")
                .arg("-cf")
                .arg(&archive)
                .arg("--exclude=./.git")
                .arg("-C")
                .arg(w)
                .arg(".")
                .status()
                .unwrap()
                .success()
        );
        let extracted = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(extracted.path())
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            dig_work,
            crt_core::source_tree_digest(&walk_source_tree(extracted.path()).unwrap()),
            "worktree and tar-extracted tree must agree"
        );
    }
}
