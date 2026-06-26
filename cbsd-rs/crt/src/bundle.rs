// crt — assemble and commit the signed 000-RELEASE/ verification bundle.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `000-RELEASE/` bundle producer (design §8). Given a freshly built release
//! branch — the live [`Worktree`] returned by [`crate::git::materialize_branch`]
//! — this computes the `source_tree_digest`, rebuilds the patch BOM by walking
//! the branch's `Crt-Patch` trailers, writes the public bundle files, assembles
//! and **signs** `record.json`, commits the whole `000-RELEASE/` directory in a
//! single commit, and creates an annotated tag carrying the manifest digest.
//!
//! **Signed-by-construction (§8):** `bundle_digests` carries a `sha256` of every
//! other bundle file and `record.json.asc` signs `record.json`, so the single
//! detached signature transitively covers the whole bundle. The `000-RELEASE/`
//! commit is therefore created **once**, with `record.json.asc` already in it;
//! signing happens before that commit.
//!
//! Subprocess `git` + filesystem IO + signing — **blocking**; a caller under an
//! async runtime must offload it (e.g. `tokio::task::spawn_blocking`).

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use crt_core::{
    MATERIALIZATION_RECORD_VERSION, MaterializationRecord, MaterializedPatch, RenderSpec, Sha256,
};

use crate::git::Worktree;
use crate::verify::{BUNDLE_DIR, RECORD_JSON, RECORD_SIG, walk_source_tree};

/// Everything the async caller pre-computes (store + pure `crt-core` work) so
/// the blocking bundle step touches only the worktree, the filesystem, and the
/// signing key. The public bundle file *bytes* are computed once by the caller
/// (so the optional loose `--out` copy and the signed bundle copy cannot drift).
pub struct BundleInputs {
    /// Public bundle files keyed by basename — `sbom.cdx.json`,
    /// `RELEASE-NOTES.md`, `provenance.json`, `README.md`, `.gitattributes`.
    /// **Not** `record.json` / `record.json.asc` (assembled and signed here).
    pub files: BTreeMap<String, Vec<u8>>,
    /// Back-reference to the sealed release (its manifest digest).
    pub s3_manifest_digest: Sha256,
    pub base_ref: String,
    /// Materialize time (RFC 3339), sourced by the caller (`crt-core` is pure).
    pub created: String,
    pub render: RenderSpec,
    /// `blob_hash` hex → (`order`, `patch_id`) from the sealed entries. The BOM
    /// joins each trailer-walked commit to the sealed `patch_id` — the
    /// import-time, offset-invariant id that leg 3 (M4 4.4) re-derives from the
    /// commit diff and compares against. Sourcing it from the entry (not the
    /// commit) keeps that later check meaningful rather than tautological.
    pub entries: BTreeMap<String, (u32, String)>,
    /// Armored signing key (fetched from Vault by the caller) + optional pass.
    pub signing_key_armored: String,
    pub passphrase: Option<String>,
    /// Carried by the annotated tag's message; equals `s3_manifest_digest`.
    pub manifest_digest: Sha256,
    /// Annotated tag name (the release name).
    pub tag: String,
}

/// What the bundle step produced, for the caller to report.
pub struct BundleResult {
    pub tag: String,
    pub bundle_commit: String,
}

/// Append the signed `000-RELEASE/` commit and the annotated tag to the release
/// branch held by `wt`, then tear the worktree down. `push` is `Some(remote)` to
/// also publish the branch + tag (opt-in, network). On any failure the worktree
/// is removed and the branch + tag deleted (fail-loud, design §8).
///
/// Blocking — call under `spawn_blocking`.
pub fn write_bundle(
    wt: Worktree,
    inputs: BundleInputs,
    push: Option<&str>,
) -> Result<BundleResult> {
    match write_bundle_inner(&wt, &inputs, push) {
        Ok(result) => {
            // The release is fully materialized (branch, signed bundle, tag,
            // and any push all landed). A failure to remove the *scratch*
            // worktree now must not report the whole operation as failed: the
            // artifacts are intact. Warn and leave the harmless scratch
            // worktree for manual cleanup rather than returning an error.
            if let Err(e) = wt.remove() {
                eprintln!(
                    "warning: {} materialized, but removing the scratch worktree failed: {e:#}",
                    inputs.tag
                );
            }
            Ok(result)
        }
        Err(e) => {
            wt.cleanup_failed(Some(&inputs.tag));
            Err(e)
        }
    }
}

fn write_bundle_inner(
    wt: &Worktree,
    inputs: &BundleInputs,
    push: Option<&str>,
) -> Result<BundleResult> {
    // 1. Hash the patched source before `000-RELEASE/` is written. (The digest
    //    excludes `000-RELEASE/` regardless, but a recipient excludes it too, so
    //    this is the pre-bundle source tree both sides agree on.)
    let source_tree_digest = crt_core::source_tree_digest(&walk_source_tree(wt.path())?);

    // 2. Rebuild the BOM from the branch's `Crt-Patch` trailers (no carried
    //    apply state): each commit's trailer names its blob, joined to the sealed
    //    entry's `order` + `patch_id`.
    let patches = walk_branch_bom(wt, &inputs.base_ref, &inputs.entries)?;

    // 3. Write the public bundle files and hash each into `bundle_digests`.
    let bundle_dir = wt.path().join(BUNDLE_DIR);
    std::fs::create_dir(&bundle_dir)
        .with_context(|| format!("creating {}", bundle_dir.display()))?;
    let mut bundle_digests = BTreeMap::new();
    for (name, bytes) in &inputs.files {
        std::fs::write(bundle_dir.join(name), bytes)
            .with_context(|| format!("writing {BUNDLE_DIR}/{name}"))?;
        bundle_digests.insert(name.clone(), Sha256::of(bytes));
    }

    // 4. Assemble the record, serialize ONCE, and sign those exact bytes — the
    //    verifier reads `record.json` verbatim (no re-serialization), so producer
    //    and verifier must agree on the literal bytes.
    let record = MaterializationRecord {
        schema_version: MATERIALIZATION_RECORD_VERSION,
        s3_manifest_digest: inputs.s3_manifest_digest,
        base_ref: inputs.base_ref.clone(),
        created: inputs.created.clone(),
        render: inputs.render.clone(),
        source_tree_digest,
        bundle_digests,
        patches,
    };
    let record_bytes = serde_json::to_vec_pretty(&record).context("serializing record.json")?;
    let signature = crt_core::sign_manifest(
        rand::thread_rng(),
        &record_bytes,
        &inputs.signing_key_armored,
        inputs.passphrase.as_deref(),
    )?;
    std::fs::write(bundle_dir.join(RECORD_JSON), &record_bytes)
        .with_context(|| format!("writing {BUNDLE_DIR}/{RECORD_JSON}"))?;
    std::fs::write(bundle_dir.join(RECORD_SIG), &signature.0)
        .with_context(|| format!("writing {BUNDLE_DIR}/{RECORD_SIG}"))?;

    // 5. Commit the whole bundle directory in one commit (`record.json.asc`
    //    included), so the single signature covers everything that lands.
    wt.git(&["add", BUNDLE_DIR])?;
    wt.git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        &format!(
            "{BUNDLE_DIR}: signed CRT verification bundle for {}",
            inputs.tag
        ),
    ])
    .context("committing the 000-RELEASE bundle")?;
    let bundle_commit = wt.git(&["rev-parse", "HEAD"])?.trim().to_owned();

    // 6. Annotated tag at the bundle tip, carrying the manifest digest. Not
    //    GPG-signed here (a signed tag over the git graph is §8 future work).
    wt.git(&[
        "-c",
        "tag.gpgsign=false",
        "tag",
        "-a",
        &inputs.tag,
        "-m",
        &inputs.manifest_digest.to_hex(),
    ])
    .with_context(|| format!("creating annotated tag {}", inputs.tag))?;

    // 7. Optional publish (opt-in, network). `--atomic` so the branch and tag
    //    land together or not at all — no partial remote state.
    if let Some(remote) = push {
        wt.git(&[
            "push",
            "--atomic",
            remote,
            wt.branch(),
            &format!("refs/tags/{}", inputs.tag),
        ])
        .with_context(|| format!("pushing {} + tag {} to {remote}", wt.branch(), inputs.tag))?;
    }

    Ok(BundleResult {
        tag: inputs.tag.clone(),
        bundle_commit,
    })
}

/// Walk `base_ref..HEAD` (apply order, oldest first) rebuilding the patch BOM
/// from each commit's `Crt-Patch` trailer, joined to the sealed entry's `order`
/// and `patch_id`. A commit without a `Crt-Patch` trailer, or whose blob matches
/// no sealed entry, is a hard error (fail-loud).
fn walk_branch_bom(
    wt: &Worktree,
    base_ref: &str,
    entries: &BTreeMap<String, (u32, String)>,
) -> Result<Vec<MaterializedPatch>> {
    let range = format!("{base_ref}..HEAD");
    let log = wt.git(&["log", "--reverse", "--format=%H", &range])?;
    let mut bom = Vec::new();
    for sha in log.split_whitespace() {
        let trailer = wt.git(&[
            "log",
            "-1",
            "--format=%(trailers:key=Crt-Patch,valueonly)",
            sha,
        ])?;
        let blob_hex = trailer
            .trim()
            .strip_prefix("sha256:")
            .map(str::to_owned)
            .with_context(|| format!("commit {sha} has no Crt-Patch trailer"))?;
        let (order, patch_id) = entries.get(&blob_hex).with_context(|| {
            format!("commit {sha} Crt-Patch {blob_hex} matches no sealed entry")
        })?;
        bom.push(MaterializedPatch {
            order: *order,
            blob_hash: Sha256::try_from(blob_hex.clone())?,
            patch_id: patch_id.clone(),
            git_commit: sha.to_owned(),
        });
    }
    Ok(bom)
}
