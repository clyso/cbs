// crt — draft release authoring (`release new` / `add` / `info`).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Author a store-backed draft release (design §3/§5, plan M2.4). A draft is
//! created by `new`, populated with patch entries by `add`, and inspected by
//! `info`. Drafts live in the shared store (not on local disk) so any operator
//! can pick up in-progress work; `seal` (M2.5) consumes one. Two read paths over
//! a *sealed* release also live here: `info` (human summary) and `notes`
//! ([`render_sealed_notes`] — the §7.2 projection, re-rendered from the pinned
//! `RenderSpec`).
//!
//! Entry metadata is authored with flags; the narrative fields
//! (`public_summary` / `behavior_change` / `upgrade_notes`) come from flags or,
//! when `--public-summary` is omitted, from a single `$EDITOR` session. The
//! pure helpers (resolution lives in [`crate::config`]; entry construction,
//! editor-buffer parsing, and rendering live here) are unit-tested; the IO
//! shims (`$EDITOR`, `git config` author lookup) are thin.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use crt_core::{
    Blast, Conflict, Coverage, Draft, Identity, Justification, JustificationKind, KnownIssue,
    Lifecycle, Manifest, ManifestEntry, PatchMeta, PatchStatus, ReleaseHeader, RenderSpec, Risk,
    Sha256, Visibility,
};
use crt_store::Store;

use crate::config::Config;

/// The notes template sealed into every release. Its bytes are digested into
/// `RenderSpec` and stored content-addressed under the same digest; `release
/// notes` / `release materialize` fetch it back by that digest and render it
/// (design §7.2). The pinned `minijinja` version lives in `crt-core`
/// ([`crt_core::RENDER_MINIJINJA_VERSION`]) — the crate that links the engine.
const DEFAULT_NOTES_TEMPLATE: &str = include_str!("../assets/default-release-notes.md.j2");

/// clap mirror of [`Visibility`] (keeps `clap` out of the pure `crt-core`).
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum VisibilityArg {
    Public,
    Private,
}

/// clap mirror of [`Blast`].
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum BlastArg {
    Cosmetic,
    Availability,
    DataLoss,
}

/// clap mirror of [`Conflict`].
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ConflictArg {
    Clean,
    Trivial,
    Substantive,
}

/// clap mirror of [`Coverage`].
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum CoverageArg {
    Strong,
    Partial,
    Weak,
}

/// clap mirror of [`JustificationKind`].
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum JustificationArg {
    Cve,
    Customer,
    Engineering,
}

impl From<VisibilityArg> for Visibility {
    fn from(v: VisibilityArg) -> Self {
        match v {
            VisibilityArg::Public => Visibility::Public,
            VisibilityArg::Private => Visibility::Private,
        }
    }
}

impl From<BlastArg> for Blast {
    fn from(b: BlastArg) -> Self {
        match b {
            BlastArg::Cosmetic => Blast::Cosmetic,
            BlastArg::Availability => Blast::Availability,
            BlastArg::DataLoss => Blast::DataLoss,
        }
    }
}

impl From<ConflictArg> for Conflict {
    fn from(c: ConflictArg) -> Self {
        match c {
            ConflictArg::Clean => Conflict::Clean,
            ConflictArg::Trivial => Conflict::Trivial,
            ConflictArg::Substantive => Conflict::Substantive,
        }
    }
}

impl From<CoverageArg> for Coverage {
    fn from(c: CoverageArg) -> Self {
        match c {
            CoverageArg::Strong => Coverage::Strong,
            CoverageArg::Partial => Coverage::Partial,
            CoverageArg::Weak => Coverage::Weak,
        }
    }
}

impl From<JustificationArg> for JustificationKind {
    fn from(j: JustificationArg) -> Self {
        match j {
            JustificationArg::Cve => JustificationKind::Cve,
            JustificationArg::Customer => JustificationKind::Customer,
            JustificationArg::Engineering => JustificationKind::Engineering,
        }
    }
}

/// The fully-resolved metadata applied to every entry added in one `add`
/// invocation. The narrative fields are already resolved (flags or `$EDITOR`),
/// so building entries from this is pure.
pub struct EntryFields {
    pub visibility: Visibility,
    pub category: String,
    pub component: String,
    pub blast: Blast,
    pub conflict: Conflict,
    pub coverage: Coverage,
    pub kind: JustificationKind,
    pub refs: Vec<String>,
    pub public_summary: String,
    pub internal: Option<String>,
    pub behavior_change: Option<String>,
    pub upgrade_notes: Option<String>,
}

/// What `add_entries` did, for the caller to report.
pub struct AddResult {
    pub added: Vec<Sha256>,
    /// Blobs already present in the draft (re-runs are idempotent).
    pub skipped: Vec<Sha256>,
}

/// Build a [`ManifestEntry`] from an imported patch's `meta`, the shared
/// `fields`, and an apply `order`. `patch_id` and `provenance` are
/// denormalized from the patch metadata (design §3); `lifecycle` starts
/// `active` with no `first_shipped_in` (cross-release tracking is later work).
fn build_entry(
    blob_hash: Sha256,
    meta: &PatchMeta,
    order: u32,
    fields: &EntryFields,
) -> ManifestEntry {
    ManifestEntry {
        blob_hash,
        patch_id: meta.patch_id.clone(),
        order,
        visibility: fields.visibility,
        category: fields.category.clone(),
        risk: Risk {
            component: fields.component.clone(),
            blast: fields.blast,
            conflict: fields.conflict,
            coverage: fields.coverage,
        },
        justification: Justification {
            kind: fields.kind,
            refs: fields.refs.clone(),
            public_summary: fields.public_summary.clone(),
            internal: fields.internal.clone(),
        },
        behavior_change: fields.behavior_change.clone(),
        upgrade_notes: fields.upgrade_notes.clone(),
        lifecycle: Lifecycle {
            status: PatchStatus::Active,
            first_shipped_in: None,
        },
        data_structure_change: None,
        provenance: meta.provenance.clone(),
    }
}

/// `crt release new <name>`: resolve the name to a channel, then write a fresh,
/// empty draft into the store — **refusing** to clobber an existing draft or
/// sealed release for the same key (the store-backed, collaborative model means
/// a blind overwrite would wipe a colleague's in-progress work).
pub async fn new_release(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
    base_ref: &str,
    author: Identity,
    created: String,
) -> Result<crt_core::ReleaseKey> {
    let key = cfg.resolve_release_key(name)?;

    if exists(store.get_draft(&key).await)? {
        bail!("a draft named {name:?} already exists; refusing to overwrite it");
    }
    if exists(store.get_release(&key).await)? {
        bail!("a sealed release named {name:?} already exists (releases are write-once)");
    }

    let draft = Draft {
        release: ReleaseHeader {
            product: cfg.component.clone(),
            namespace: key.namespace.clone(),
            channel: key.channel.clone(),
            name: name.to_owned(),
            base_ref: base_ref.to_owned(),
            created,
            author,
        },
        entries: vec![],
        known_issues: vec![],
        upgrade_notes: None,
    };
    store.put_draft(&key, &draft).await?;
    Ok(key)
}

/// Collapse a store read into "does it exist?": `Ok` ⇒ present, a not-found
/// error ⇒ absent, any other error propagates.
fn exists<T>(read: std::result::Result<T, crt_store::StoreError>) -> Result<bool> {
    match read {
        Ok(_) => Ok(true),
        Err(e) if e.is_not_found() => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// `crt release add <name> <blob_hash…>`: append an entry per blob to the
/// draft, applying `fields` to each. Blobs already in the draft are skipped
/// (idempotent re-runs); a blob with no stored metadata is an error (import it
/// first). Entries are written back in one `put_draft`.
pub async fn add_entries(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
    blobs: &[String],
    fields: &EntryFields,
) -> Result<AddResult> {
    cfg.validate_risk_component(&fields.component)?;
    let key = cfg.resolve_release_key(name)?;
    let mut draft = store
        .get_draft(&key)
        .await
        .with_context(|| format!("no draft named {name:?}; run `crt release new` first"))?;

    let mut next_order = draft
        .entries
        .iter()
        .map(|e| e.order)
        .max()
        .map_or(1, |m| m + 1);
    let mut result = AddResult {
        added: vec![],
        skipped: vec![],
    };
    for blob in blobs {
        let hash = Sha256::try_from(blob.clone())
            .map_err(|_| anyhow!("{blob:?} is not a valid 64-char hex sha256 blob hash"))?;
        if draft.entries.iter().any(|e| e.blob_hash == hash) {
            result.skipped.push(hash);
            continue;
        }
        let meta = store.get_meta(&hash).await.map_err(|e| {
            if e.is_not_found() {
                anyhow!("blob {hash} has no metadata in the store — import it first with `crt patch import`")
            } else {
                e.into()
            }
        })?;
        draft
            .entries
            .push(build_entry(hash, &meta, next_order, fields));
        next_order += 1;
        result.added.push(hash);
    }

    if !result.added.is_empty() {
        store.put_draft(&key, &draft).await?;
    }
    Ok(result)
}

/// `crt release info <name>`: render the draft for `name`, or — if no draft
/// exists — the sealed release. Errors only if neither is present.
pub async fn show_info(store: &dyn Store, cfg: &Config, name: &str) -> Result<String> {
    let key = cfg.resolve_release_key(name)?;
    match store.get_draft(&key).await {
        Ok(draft) => {
            let display = cfg
                .namespaces
                .get(&key.namespace)
                .and_then(|ns| ns.channels.get(&key.channel))
                .map_or("", |c| c.branding.display_name.as_str());
            Ok(render_info(
                "draft",
                &draft.release,
                &draft.entries,
                &draft.known_issues,
                draft.upgrade_notes.as_deref(),
                display,
            ))
        }
        Err(e) if e.is_not_found() => match store.get_release(&key).await {
            Ok(rec) => Ok(render_info(
                "sealed",
                &rec.manifest.release,
                &rec.manifest.entries,
                &rec.manifest.known_issues,
                rec.manifest.upgrade_notes.as_deref(),
                &rec.manifest.branding.display_name,
            )),
            Err(e) if e.is_not_found() => {
                bail!("no draft or sealed release named {name:?}")
            }
            Err(e) => Err(e.into()),
        },
        Err(e) => Err(e.into()),
    }
}

/// `crt release seal <name>`: turn a draft into a signed, write-once
/// `ReleaseRecord` (design §6). The signing key bytes are **injected** (fetched
/// from Vault by the caller — `crt-core` and this pipeline never touch Vault),
/// so the whole seal path is unit-testable with a generated key.
///
/// The step order is load-bearing: the manifest is canonicalized, digested, and
/// **signed before** the write-once `put_release`, so a Vault/sign failure never
/// burns the write-once key with a half-sealed record; and the draft is deleted
/// **last**, only once the sealed record has landed, so a failed seal leaves the
/// draft intact for retry or handoff.
pub async fn seal_release<R: rand::Rng + rand::CryptoRng>(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
    secret_key_armored: &str,
    passphrase: Option<&str>,
    rng: R,
) -> Result<crt_core::ReleaseKey> {
    let key = cfg.resolve_release_key(name)?;

    let draft = store
        .get_draft(&key)
        .await
        .with_context(|| format!("no draft named {name:?} to seal; run `crt release new` first"))?;
    if draft.entries.is_empty() {
        bail!("draft {name:?} has no entries; add patches before sealing");
    }

    // Snapshot branding from the draft's *stored* namespace/channel — not a
    // re-resolution of the name, which could pick a different channel if config
    // drifted since `new`. Branding must be configured: sealing empty branding
    // into a signed manifest is permanent, so a missing channel is a hard error.
    let branding = cfg
        .namespaces
        .get(&draft.release.namespace)
        .and_then(|ns| ns.channels.get(&draft.release.channel))
        .map(|c| c.branding.clone())
        .with_context(|| {
            format!(
                "no branding configured for {}/{}; cannot seal",
                draft.release.namespace, draft.release.channel
            )
        })?;

    let template_digest = Sha256::of(DEFAULT_NOTES_TEMPLATE.as_bytes());
    let manifest = Manifest {
        schema_version: crt_core::SCHEMA_VERSION,
        release: draft.release.clone(),
        entries: draft.entries.clone(),
        known_issues: draft.known_issues.clone(),
        upgrade_notes: draft.upgrade_notes.clone(),
        branding,
        render: RenderSpec {
            minijinja_version: crt_core::RENDER_MINIJINJA_VERSION.to_owned(),
            template_digest,
        },
    };

    // Canonicalize once: the digest and the signature cover the exact same bytes.
    let canonical = crt_core::canonical_json(&manifest)?;
    let digest = Sha256::of(&canonical);
    let signature = crt_core::sign_manifest(rng, &canonical, secret_key_armored, passphrase)?;
    let record = crt_core::ReleaseRecord {
        manifest,
        digest,
        signature,
    };

    // Store the template (content-addressed, idempotent) just before the
    // write-once release, so an earlier failure leaves no orphan.
    store
        .put_template(&template_digest, DEFAULT_NOTES_TEMPLATE.as_bytes())
        .await?;
    store.put_release(&key, &record).await?;
    store.delete_draft(&key).await?;
    Ok(key)
}

/// `crt release list`: the keys of all sealed releases, sorted for stable
/// output.
pub async fn list_releases(store: &dyn Store) -> Result<Vec<crt_core::ReleaseKey>> {
    let mut keys = store.list_releases().await?;
    keys.sort_by(|a, b| {
        (&a.namespace, &a.channel, &a.name).cmp(&(&b.namespace, &b.channel, &b.name))
    });
    Ok(keys)
}

/// `crt release notes <name>`: re-render the canonical release notes for a
/// **sealed** release from its pinned `RenderSpec` (design §7.2) — no re-seal.
/// All three rendering inputs are pinned: the branding snapshot (in the
/// manifest), the `minijinja` version (gated against this build's linked
/// version), and the template (fetched from the store by its sealed digest).
/// Errors if there is no sealed release for `name`, or the pinned `minijinja`
/// version does not match this build — we refuse to silently re-render with a
/// different engine, whose bytes could diverge from the sealed notes.
pub async fn render_sealed_notes(store: &dyn Store, cfg: &Config, name: &str) -> Result<String> {
    Ok(sealed_record_and_notes(store, cfg, name).await?.1)
}

/// Load a sealed release and render its notes — the shared core of `release
/// notes` and `release materialize`. Returns the record (whose manifest also
/// feeds the SBOM) alongside the rendered notes. Errors if no sealed release
/// exists or the pinned `minijinja` version does not match.
async fn sealed_record_and_notes(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
) -> Result<(crt_core::ReleaseRecord, String)> {
    let key = cfg.resolve_release_key(name)?;
    let record = store.get_release(&key).await.with_context(|| {
        format!(
            "no sealed release named {name:?} \
             (notes render from a sealed release; seal it first)"
        )
    })?;
    let notes = render_notes_for(store, &record).await?;
    Ok((record, notes))
}

/// Render a sealed release's notes from an already-loaded record (the shared
/// core of [`sealed_record_and_notes`] and `verify`'s leg 4). Version-gates the
/// sealed `RenderSpec`, loads the template by its sealed digest, and renders —
/// so a re-render is byte-identical to the materialized notes. Errors if the
/// pinned `minijinja` version does not match this build.
pub(crate) async fn render_notes_for(
    store: &dyn Store,
    record: &crt_core::ReleaseRecord,
) -> Result<String> {
    crt_core::check_render_version(&record.manifest.render)?;

    let template_bytes = store
        .get_template(&record.manifest.render.template_digest)
        .await
        .with_context(|| {
            format!(
                "loading the sealed notes template {}",
                record.manifest.render.template_digest
            )
        })?;
    let template = std::str::from_utf8(&template_bytes)
        .context("the sealed notes template is not valid UTF-8")?;

    Ok(crt_core::render_notes(&record.manifest, template)?)
}

/// The loose artifact files written by [`write_loose_artifacts`].
#[derive(Debug)]
pub struct MaterializeOutput {
    pub notes: PathBuf,
    pub sbom: PathBuf,
}

/// Project a sealed release into its two deterministic artifacts **once**: the
/// rendered `RELEASE-NOTES.md` and the CycloneDX `sbom.cdx.json`. Both the loose
/// `--out` emit and the signed `000-RELEASE/` bundle consume these same bytes,
/// so the two copies cannot drift. Version-gates the sealed `RenderSpec` (via
/// `sealed_record_and_notes`). Returns the record alongside the bytes.
async fn prepare_artifacts(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
) -> Result<(crt_core::ReleaseRecord, String, String)> {
    let (record, notes) = sealed_record_and_notes(store, cfg, name).await?;
    let sbom = crt_core::build_sbom(&record.manifest)?;
    Ok((record, notes, sbom))
}

/// Write the loose `RELEASE-NOTES.md` + `sbom.cdx.json` into `out_dir` (created
/// if absent), overwriting any existing copies. The bytes are pure projections,
/// so a re-run is byte-identical.
async fn write_loose_artifacts(
    out_dir: &Path,
    notes: &str,
    sbom: &str,
) -> Result<MaterializeOutput> {
    // Async filesystem IO: this runs under tokio, so blocking `std::fs` would
    // stall the executor (rust-2024). `tokio::fs` offloads to the blocking pool.
    tokio::fs::create_dir_all(out_dir)
        .await
        .with_context(|| format!("creating the output directory {}", out_dir.display()))?;
    let notes_path = out_dir.join("RELEASE-NOTES.md");
    let sbom_path = out_dir.join("sbom.cdx.json");
    tokio::fs::write(&notes_path, notes)
        .await
        .with_context(|| format!("writing {}", notes_path.display()))?;
    tokio::fs::write(&sbom_path, sbom)
        .await
        .with_context(|| format!("writing {}", sbom_path.display()))?;
    Ok(MaterializeOutput {
        notes: notes_path,
        sbom: sbom_path,
    })
}

/// What `release materialize` produced, for the caller to report.
#[derive(Debug)]
pub struct MaterializeSummary {
    /// The `release/<name>` branch built in the destination repo.
    pub branch: String,
    /// The patch commit hashes, in apply order (pre-bundle).
    pub commits: Vec<String>,
    /// The annotated tag carrying the manifest digest.
    pub tag: String,
    /// The `000-RELEASE/` bundle commit at the branch tip.
    pub bundle_commit: String,
    /// The loose artifact files, when `--out` requested them (decision 4).
    pub loose: Option<MaterializeOutput>,
}

/// `crt release materialize <name>`: build the linear `release/<name>` branch in
/// the destination repo from the sealed release, then append the signed
/// `000-RELEASE/` verification bundle commit and an annotated tag carrying the
/// manifest digest (design §8). `git am` applies each entry's patch blob in
/// `order`, each commit carrying a `Crt-Patch` trailer; the bundle's
/// `record.json` is signed with the **injected** Vault key (this pipeline never
/// touches Vault). With `out`, the notes + SBOM are *also* emitted there as
/// loose files (decision 4) from the same bytes the bundle uses. With `push`,
/// the branch + tag are published to `origin` (opt-in, network).
///
/// The signing key bytes are injected and `created` is sourced by the caller, so
/// the whole path is unit-testable with a generated key and an
/// `object_store::InMemory` store (`crt-core` stays clock- and Vault-free).
#[allow(clippy::too_many_arguments)]
pub async fn materialize(
    store: &dyn Store,
    cfg: &Config,
    name: &str,
    repo: Option<&Path>,
    out: Option<&Path>,
    signing_key_armored: &str,
    passphrase: Option<&str>,
    created: String,
    push: bool,
    token: Option<&str>,
) -> Result<MaterializeSummary> {
    // Project the sealed release once; both the loose `--out` copy and the
    // bundle consume these exact bytes (no drift).
    let (record, notes, sbom) = prepare_artifacts(store, cfg, name).await?;

    let loose = match out {
        Some(dir) => Some(write_loose_artifacts(dir, &notes, &sbom).await?),
        None => None,
    };

    // Resolve the destination repo: `--repo <path>` is the local working copy;
    // the configured `destination_repo` slug is the fallback (used for push).
    let repo_path: PathBuf = repo
        .map(Path::to_path_buf)
        .or_else(|| cfg.destination_repo.as_ref().map(PathBuf::from))
        .context(
            "no destination repo: pass --repo <path> or set `destination_repo` in the config",
        )?;

    // Collect each entry's patch blob in `order` for `git am`, plus the
    // blob_hash → (order, patch_id) join the bundle BOM walk needs. Entries are
    // normally appended in order; sort defensively.
    let mut entries: Vec<&ManifestEntry> = record.manifest.entries.iter().collect();
    entries.sort_by_key(|e| e.order);
    let mut patches = Vec::with_capacity(entries.len());
    let mut entry_map: BTreeMap<String, (u32, String)> = BTreeMap::new();
    for e in &entries {
        let bytes = store
            .get_blob(&e.blob_hash)
            .await
            .with_context(|| format!("fetching patch blob {} (order {})", e.blob_hash, e.order))?;
        patches.push(crate::git::PatchToApply {
            order: e.order,
            blob_hash: e.blob_hash.to_hex(),
            bytes: bytes.to_vec(),
        });
        entry_map.insert(e.blob_hash.to_hex(), (e.order, e.patch_id.clone()));
    }

    // The public bundle files (besides record.json/.asc), built from the same
    // projection bytes. `provenance.json` is a public-safe projection — no
    // visibility, no internal justification.
    let provenance = crt_core::PublicProvenance::from_entries(&record.manifest.entries);
    let provenance_bytes =
        serde_json::to_vec_pretty(&provenance).context("serializing provenance.json")?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("sbom.cdx.json".to_owned(), sbom.into_bytes());
    files.insert("RELEASE-NOTES.md".to_owned(), notes.into_bytes());
    files.insert("provenance.json".to_owned(), provenance_bytes);
    files.insert("README.md".to_owned(), bundle_readme(name).into_bytes());
    // Inside `000-RELEASE/`, a slash-free `* -text` matches every bundle file in
    // that directory, so the signed files are never EOL-mangled on checkout. A
    // pattern *with* a slash (`000-RELEASE/* -text`) would resolve relative to
    // this directory and match nothing (verified with `git check-attr`).
    files.insert(".gitattributes".to_owned(), b"* -text\n".to_vec());

    let branch = format!("release/{name}");
    let inputs = crate::bundle::BundleInputs {
        files,
        s3_manifest_digest: record.digest,
        base_ref: record.manifest.release.base_ref.clone(),
        created,
        render: record.manifest.render.clone(),
        entries: entry_map,
        signing_key_armored: signing_key_armored.to_owned(),
        passphrase: passphrase.map(str::to_owned),
        manifest_digest: record.digest,
        tag: name.to_owned(),
    };

    // Subprocess git + filesystem IO + signing are blocking; offload so they
    // never stall the async executor (rust-2024). Owned inputs move into the
    // task.
    let (commits, result) = {
        let repo_path = repo_path.clone();
        let branch = branch.clone();
        let base_ref = record.manifest.release.base_ref.clone();
        let push_remote = if push {
            Some("origin".to_owned())
        } else {
            None
        };
        // Own the token for the 'static blocking task.
        let token = token.map(str::to_owned);
        tokio::task::spawn_blocking(
            move || -> Result<(Vec<String>, crate::bundle::BundleResult)> {
                // Refuse up front if the tag already exists, before building
                // anything. The branch has an implicit guard (`git worktree add
                // -b` fails on an existing branch); the tag needs an explicit
                // one, else a later failure's `cleanup_failed` could delete a
                // tag this run did not create.
                if crate::git::tag_exists(&repo_path, &inputs.tag)? {
                    anyhow::bail!(
                        "tag {:?} already exists in the destination repo \
                         (releases are write-once)",
                        inputs.tag
                    );
                }
                let (wt, commits) =
                    crate::git::materialize_branch(&repo_path, &branch, &base_ref, &patches)?;
                let result = crate::bundle::write_bundle(
                    wt,
                    inputs,
                    push_remote.as_deref(),
                    token.as_deref(),
                )?;
                Ok((commits, result))
            },
        )
        .await
        .context("the git materialization task panicked")??
    };

    Ok(MaterializeSummary {
        branch,
        commits,
        tag: result.tag,
        bundle_commit: result.bundle_commit,
        loose,
    })
}

/// The `000-RELEASE/README.md`: what the bundle is and how to verify it offline.
/// A pure function of the release `name` (deterministic).
fn bundle_readme(name: &str) -> String {
    format!(
        "# 000-RELEASE — CRT verification bundle\n\
         \n\
         This directory is the portable, signed verification bundle for the Clyso\n\
         downstream Ceph release `{name}` (Ceph Release Tool, design §8).\n\
         \n\
         ## Contents\n\
         \n\
         - `record.json` — the signed materialization record: the source-tree\n\
         digest, the per-file bundle digests, the patch BOM, and a back-reference\n\
         to the sealed manifest.\n\
         - `record.json.asc` — detached OpenPGP signature over `record.json`. It\n\
         transitively covers every other file here, since `record.json` lists\n\
         their digests.\n\
         - `sbom.cdx.json` — CycloneDX SBOM of the release.\n\
         - `RELEASE-NOTES.md` — the rendered release notes.\n\
         - `provenance.json` — public per-patch provenance.\n\
         - `.gitattributes` — keeps these files byte-exact across checkout.\n\
         \n\
         ## Verify offline\n\
         \n\
         ```\n\
         crt verify --tree . --public-key <clyso-public-key>\n\
         ```\n\
         \n\
         This recomputes the source-tree and bundle digests and checks the\n\
         signature — no network, no git history, and no S3 access required.\n"
    )
}

/// Render a human-readable summary of a draft or sealed release. Pure: risk
/// totals/bands are computed from the entries (concept §6.1). Shows
/// `public_summary` only — `justification.internal` is never rendered.
fn render_info(
    kind: &str,
    header: &ReleaseHeader,
    entries: &[ManifestEntry],
    known_issues: &[KnownIssue],
    upgrade_notes: Option<&str>,
    display_name: &str,
) -> String {
    let mut s = String::new();
    s.push_str(&format!("{kind}  {}\n", header.name));
    s.push_str(&format!("  namespace  {}\n", header.namespace));
    s.push_str(&format!(
        "  channel    {}  ({display_name})\n",
        header.channel
    ));
    s.push_str(&format!("  product    {}\n", header.product));
    s.push_str(&format!("  base ref   {}\n", header.base_ref));
    s.push_str(&format!("  created    {}\n", header.created));
    s.push_str(&format!(
        "  author     {} <{}>\n",
        header.author.name, header.author.email
    ));
    s.push_str(&format!("  entries    {}\n", entries.len()));
    for e in entries {
        let band = format!("{:?}", e.risk_band()).to_lowercase();
        let vis = format!("{:?}", e.visibility).to_lowercase();
        let summary = e.justification.public_summary.lines().next().unwrap_or("");
        s.push_str(&format!(
            "    [{}] {} {:<9} {:<8} {:<12} risk {} {:<7} {}\n",
            e.order,
            &e.blob_hash.to_hex()[..12],
            e.category,
            vis,
            e.risk.component,
            e.risk_total(),
            band,
            summary,
        ));
    }
    if !known_issues.is_empty() {
        s.push_str(&format!("  known issues {}\n", known_issues.len()));
        for ki in known_issues {
            s.push_str(&format!("    - {}\n", ki.summary));
        }
    }
    if let Some(notes) = upgrade_notes {
        s.push_str("  upgrade notes:\n");
        for line in notes.lines() {
            s.push_str(&format!("    {line}\n"));
        }
    }
    s
}

/// Resolve the release author: explicit `--author-name`/`--author-email` take
/// precedence; missing parts fall back to the effective `git config`
/// (`user.name` / `user.email`). Errors if a part is neither given nor
/// configured.
pub fn resolve_author(name: Option<String>, email: Option<String>) -> Result<Identity> {
    let name = match name {
        Some(n) => n,
        None => git_config("user.name")
            .context("no --author-name and `git config user.name` is unset")?,
    };
    let email = match email {
        Some(e) => e,
        None => git_config("user.email")
            .context("no --author-email and `git config user.email` is unset")?,
    };
    Ok(Identity { name, email })
}

/// Read a single `git config <key>` value (effective config) from the current
/// directory. Errors if the key is unset or empty.
fn git_config(key: &str) -> Result<String> {
    let cwd = std::env::current_dir().context("resolving the current directory")?;
    let value = crate::git::git(&cwd, &["config", key])?.trim().to_owned();
    if value.is_empty() {
        bail!("git config {key} is empty");
    }
    Ok(value)
}

/// Compose the narrative fields in one `$EDITOR` session, returning
/// `(public_summary, behavior_change, upgrade_notes)`. `public_summary` is
/// required; the optional fields are `None` when left empty.
pub fn compose_via_editor() -> Result<(String, Option<String>, Option<String>)> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_owned());

    let file = tempfile::Builder::new()
        .prefix("crt-entry-")
        .suffix(".txt")
        .tempfile()
        .context("creating the editor scratch file")?;
    std::fs::write(file.path(), EDITOR_TEMPLATE).context("writing the editor template")?;

    let status = std::process::Command::new(&editor)
        .arg(file.path())
        .status()
        .with_context(|| format!("launching editor {editor:?}"))?;
    if !status.success() {
        bail!("editor {editor:?} exited with a non-zero status");
    }

    let edited = std::fs::read_to_string(file.path()).context("reading the edited entry")?;
    parse_editor_buffer(&edited)
}

const EDITOR_TEMPLATE: &str = "\
# crt: compose the release entry. Lines starting with '#' are ignored.
# Each section runs to the next '@@' marker; leave a section empty to omit it.
@@ public-summary (required; rendered into the public release notes)

@@ behavior-change (optional)

@@ upgrade-notes (optional)
";

/// Parse an `$EDITOR` buffer into `(public_summary, behavior_change,
/// upgrade_notes)`. `#` comment lines are dropped; `@@ <key> …` lines begin a
/// section keyed by `<key>`. `public-summary` must be non-empty.
fn parse_editor_buffer(text: &str) -> Result<(String, Option<String>, Option<String>)> {
    let mut current: Option<String> = None;
    let mut sections: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            let key = rest.split_whitespace().next().unwrap_or("").to_owned();
            current = Some(key.clone());
            sections.entry(key).or_default();
            continue;
        }
        if let Some(key) = &current {
            let buf = sections.entry(key.clone()).or_default();
            buf.push_str(line);
            buf.push('\n');
        }
    }

    let take = |key: &str| sections.get(key).map(|v| v.trim().to_owned());
    let public_summary = take("public-summary")
        .filter(|s| !s.is_empty())
        .context("the public-summary section is required but was left empty")?;
    let behavior_change = take("behavior-change").filter(|s| !s.is_empty());
    let upgrade_notes = take("upgrade-notes").filter(|s| !s.is_empty());
    Ok((public_summary, behavior_change, upgrade_notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt_core::{Provenance, ReleaseKey, UpstreamPrState, blob_hash};
    use crt_store::ObjectBackedStore;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::config::{Channel, Namespace, StoreConfig};

    fn test_config() -> Config {
        let mut channels = BTreeMap::new();
        channels.insert(
            "ces".to_owned(),
            Channel {
                branding: crt_core::Branding {
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
            risk_components: vec!["rgw".to_owned()],
            namespaces,
            public_key_url: None,
            gpg_private_key: None,
        }
    }

    fn test_author() -> Identity {
        Identity {
            name: "Releaser".to_owned(),
            email: "rel@example.com".to_owned(),
        }
    }

    async fn imported_meta(store: &ObjectBackedStore, body: &[u8]) -> Sha256 {
        // Synthesize an imported patch: a blob + its PatchMeta (as `patch
        // import` would have written), so `add` can denormalize from it.
        let hash = blob_hash(body);
        let meta = PatchMeta {
            blob_hash: hash,
            patch_id: format!("pid-{}", &hash.to_hex()[..8]),
            author: test_author(),
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: "Fix a thing".to_owned(),
            body: "body".to_owned(),
            cherry_picked_from: vec![],
            provenance: Provenance::UpstreamPr {
                prs: vec!["https://github.com/ceph/ceph/pull/1".to_owned()],
                commits: vec!["abc".to_owned()],
                state: UpstreamPrState::MergedMain,
            },
            source_repo: "ceph/ceph".to_owned(),
        };
        store.put_blob(&hash, body).await.unwrap();
        store.put_meta(&hash, &meta).await.unwrap();
        hash
    }

    fn fields(public_summary: &str) -> EntryFields {
        EntryFields {
            visibility: Visibility::Public,
            category: "fix".to_owned(),
            component: "rgw".to_owned(),
            blast: Blast::Availability,
            conflict: Conflict::Trivial,
            coverage: Coverage::Partial,
            kind: JustificationKind::Engineering,
            refs: vec!["https://tracker.ceph.com/issues/1".to_owned()],
            public_summary: public_summary.to_owned(),
            internal: None,
            behavior_change: None,
            upgrade_notes: None,
        }
    }

    #[tokio::test]
    async fn new_creates_a_draft_and_refuses_to_clobber() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let key = new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        assert_eq!(
            key,
            ReleaseKey {
                namespace: "clyso-enterprise".to_owned(),
                channel: "ces".to_owned(),
                name: "ces-v18.2.0".to_owned(),
            }
        );
        let draft = store.get_draft(&key).await.unwrap();
        assert_eq!(draft.release.base_ref, "v18.2.0");
        assert_eq!(draft.release.product, "ceph");
        assert!(draft.entries.is_empty());

        // A second `new` for the same name must not wipe the draft.
        assert!(
            new_release(
                &store,
                &cfg,
                "ces-v18.2.0",
                "v18.2.0",
                test_author(),
                "2026-06-22T00:00:00+00:00".to_owned(),
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn add_appends_entries_and_round_trips() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();

        let h1 = imported_meta(&store, b"patch one").await;
        let h2 = imported_meta(&store, b"patch two").await;

        // A private entry proves the visibility flag is recorded (inert).
        let mut private_fields = fields("Adds a private fix.");
        private_fields.visibility = Visibility::Private;

        let r1 = add_entries(&store, &cfg, "ces-v18.2.0", &[h1.to_hex()], &private_fields)
            .await
            .unwrap();
        assert_eq!(r1.added, vec![h1]);
        let r2 = add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h2.to_hex()],
            &fields("Public fix."),
        )
        .await
        .unwrap();
        assert_eq!(r2.added, vec![h2]);

        let draft = store
            .get_draft(&cfg.resolve_release_key("ces-v18.2.0").unwrap())
            .await
            .unwrap();
        assert_eq!(draft.entries.len(), 2);
        assert_eq!(draft.entries[0].order, 1);
        assert_eq!(draft.entries[0].visibility, Visibility::Private);
        assert_eq!(draft.entries[1].order, 2);
        assert_eq!(draft.entries[1].visibility, Visibility::Public);
        // Provenance + patch_id are denormalized from the imported meta.
        assert!(matches!(
            draft.entries[0].provenance,
            Provenance::UpstreamPr { .. }
        ));
        assert_eq!(
            draft.entries[0].patch_id,
            format!("pid-{}", &h1.to_hex()[..8])
        );
    }

    #[tokio::test]
    async fn add_skips_duplicates_and_rejects_unknown_blobs() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, b"patch one").await;

        add_entries(&store, &cfg, "ces-v18.2.0", &[h1.to_hex()], &fields("s"))
            .await
            .unwrap();
        // Re-adding the same blob is a no-op skip.
        let again = add_entries(&store, &cfg, "ces-v18.2.0", &[h1.to_hex()], &fields("s"))
            .await
            .unwrap();
        assert!(again.added.is_empty());
        assert_eq!(again.skipped, vec![h1]);

        // A blob with no stored metadata is an error.
        let orphan = blob_hash(b"never imported");
        assert!(
            add_entries(
                &store,
                &cfg,
                "ces-v18.2.0",
                &[orphan.to_hex()],
                &fields("s")
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn add_rejects_an_unconfigured_risk_component() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config(); // risk_components = ["rgw"]
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, b"patch one").await;
        let mut bad = fields("s");
        bad.component = "not-a-component".to_owned();
        assert!(
            add_entries(&store, &cfg, "ces-v18.2.0", &[h1.to_hex()], &bad)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn info_renders_a_draft_and_hides_internal_notes() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, b"patch one").await;
        let mut f = fields("Fixes a thing.");
        f.internal = Some("DO-NOT-LEAK internal note".to_owned());
        add_entries(&store, &cfg, "ces-v18.2.0", &[h1.to_hex()], &f)
            .await
            .unwrap();

        let out = show_info(&store, &cfg, "ces-v18.2.0").await.unwrap();
        assert!(out.starts_with("draft  ces-v18.2.0"));
        // The channel branding comes from config on the draft path.
        assert!(out.contains("Clyso Enterprise Storage"));
        assert!(out.contains("Fixes a thing."));
        assert!(out.contains("entries    1"));
        // `justification.internal` is an inspect-view leak risk — never render it.
        assert!(!out.contains("DO-NOT-LEAK"));

        // No draft and no release ⇒ error.
        assert!(show_info(&store, &cfg, "ces-v99.0.0").await.is_err());
    }

    #[tokio::test]
    async fn info_falls_back_to_a_sealed_release() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        // A name with a sealed release but no draft. Its branding differs from
        // config's, proving the sealed path renders the *manifest's* snapshot
        // (not the live config) — the field source the draft path doesn't use.
        let key = cfg.resolve_release_key("ces-v17.0.0").unwrap();
        let manifest = crt_core::Manifest {
            schema_version: 1,
            release: ReleaseHeader {
                product: "ceph".to_owned(),
                namespace: key.namespace.clone(),
                channel: key.channel.clone(),
                name: "ces-v17.0.0".to_owned(),
                base_ref: "v17.0.0".to_owned(),
                created: "2026-01-01T00:00:00+00:00".to_owned(),
                author: test_author(),
            },
            entries: vec![],
            known_issues: vec![],
            upgrade_notes: None,
            branding: crt_core::Branding {
                display_name: "Sealed Snapshot Brand".to_owned(),
                blurb: "b".to_owned(),
                footer: "f".to_owned(),
            },
            render: crt_core::RenderSpec {
                minijinja_version: "2.21.0".to_owned(),
                template_digest: blob_hash(b"template"),
            },
        };
        let digest = crt_core::digest(&manifest).unwrap();
        let record = crt_core::ReleaseRecord {
            manifest,
            digest,
            signature: crt_core::ArmoredSignature("-----BEGIN PGP SIGNATURE-----".to_owned()),
        };
        store.put_release(&key, &record).await.unwrap();

        let out = show_info(&store, &cfg, "ces-v17.0.0").await.unwrap();
        assert!(out.starts_with("sealed  ces-v17.0.0"));
        assert!(out.contains("Sealed Snapshot Brand"));
    }

    /// Generate a throwaway Ed25519 signing keypair (armored secret, armored
    /// public) so the seal tests exercise real signing without Vault.
    fn test_keypair() -> (String, String) {
        use pgp::composed::{ArmorOptions, KeyType, SecretKeyParamsBuilder, SignedPublicKey};
        let mut params = SecretKeyParamsBuilder::default();
        params
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(true)
            .primary_user_id("CRT Seal Test <seal@example.com>".into())
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

    /// Seed a draft with one entry, ready to seal.
    async fn draft_with_one_entry(store: &ObjectBackedStore, cfg: &Config, name: &str) {
        new_release(
            store,
            cfg,
            name,
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h = imported_meta(store, name.as_bytes()).await;
        add_entries(store, cfg, name, &[h.to_hex()], &fields("Fixes a thing."))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn seal_signs_persists_and_consumes_the_draft() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        draft_with_one_entry(&store, &cfg, "ces-v18.2.0").await;

        let (secret, public) = test_keypair();
        let key = seal_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            &secret,
            None,
            rand::thread_rng(),
        )
        .await
        .unwrap();

        // The draft is consumed; the sealed record is present.
        assert!(store.get_draft(&key).await.is_err());
        let record = store.get_release(&key).await.unwrap();

        // Digest recomputes and the signature verifies over the canonical bytes
        // (the full 2.1 + 2.2 contract, end to end).
        let canonical = crt_core::canonical_json(&record.manifest).unwrap();
        assert_eq!(record.digest, Sha256::of(&canonical));
        crt_core::verify_manifest(&canonical, &record.signature, &public)
            .expect("signature verifies against the keypair's public half");

        // Branding is the config snapshot; RenderSpec + schema_version recorded;
        // the template is stored under its sealed digest.
        assert_eq!(
            record.manifest.branding.display_name,
            "Clyso Enterprise Storage"
        );
        assert_eq!(record.manifest.schema_version, crt_core::SCHEMA_VERSION);
        assert_eq!(
            record.manifest.render.minijinja_version,
            crt_core::RENDER_MINIJINJA_VERSION
        );
        assert!(
            store
                .get_template(&record.manifest.render.template_digest)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn seal_refuses_an_empty_draft() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let (secret, _) = test_keypair();
        // A zero-entry release is almost certainly a forgotten `add`; never sign it.
        assert!(
            seal_release(
                &store,
                &cfg,
                "ces-v18.2.0",
                &secret,
                None,
                rand::thread_rng()
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn seal_refuses_when_channel_branding_is_missing() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let key = {
            draft_with_one_entry(&store, &cfg, "ces-v18.2.0").await;
            cfg.resolve_release_key("ces-v18.2.0").unwrap()
        };
        // Simulate config drift: the draft's stored channel is no longer in the
        // config, so its branding can't be resolved. Sealing empty branding into
        // a signed manifest is permanent — this must be a hard error.
        let mut draft = store.get_draft(&key).await.unwrap();
        draft.release.channel = "removed-channel".to_owned();
        store.put_draft(&key, &draft).await.unwrap();

        let (secret, _) = test_keypair();
        assert!(
            seal_release(
                &store,
                &cfg,
                "ces-v18.2.0",
                &secret,
                None,
                rand::thread_rng()
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn list_releases_returns_sealed_keys_sorted() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, _) = test_keypair();
        for name in ["ces-v18.2.1", "ces-v18.2.0"] {
            draft_with_one_entry(&store, &cfg, name).await;
            seal_release(&store, &cfg, name, &secret, None, rand::thread_rng())
                .await
                .unwrap();
        }
        let keys = list_releases(&store).await.unwrap();
        let names: Vec<_> = keys.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, vec!["ces-v18.2.0", "ces-v18.2.1"]);
    }

    #[tokio::test]
    async fn render_sealed_notes_renders_the_pinned_template_and_hides_internal() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "v18.2.0",
            test_author(),
            "2026-06-22T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h = imported_meta(&store, b"a patch").await;
        let mut f = fields("Fixes a thing.");
        f.internal = Some("DO-NOT-LEAK internal note".to_owned());
        add_entries(&store, &cfg, "ces-v18.2.0", &[h.to_hex()], &f)
            .await
            .unwrap();
        let (secret, _) = test_keypair();
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

        // The real sealed asset renders: branding snapshot + title + the public
        // summary appear; the internal note never does (design §7.2).
        let notes = render_sealed_notes(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap();
        assert!(notes.contains("Clyso Enterprise Storage — ces-v18.2.0"));
        assert!(notes.contains("Fixes a thing."));
        assert!(!notes.contains("DO-NOT-LEAK"));

        // No sealed release for the name ⇒ error.
        assert!(
            render_sealed_notes(&store, &cfg, "ces-v99.0.0")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn render_sealed_notes_covers_all_default_template_sections() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let key = cfg.resolve_release_key("ces-v18.2.0").unwrap();

        let entry =
            |order: u32, category: &str, summary: &str, behavior: Option<&str>| ManifestEntry {
                blob_hash: blob_hash(format!("p{order}").as_bytes()),
                patch_id: format!("pid-{order}"),
                order,
                visibility: Visibility::Public,
                category: category.to_owned(),
                risk: Risk {
                    component: "rgw".to_owned(),
                    blast: Blast::Availability,
                    conflict: Conflict::Trivial,
                    coverage: Coverage::Partial,
                },
                justification: Justification {
                    kind: JustificationKind::Engineering,
                    refs: vec![],
                    public_summary: summary.to_owned(),
                    internal: Some("DO-NOT-LEAK-internal".to_owned()),
                },
                behavior_change: behavior.map(str::to_owned),
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
            };

        // Hand-seal a rich record (every default-template branch) and render it.
        let template_digest = Sha256::of(DEFAULT_NOTES_TEMPLATE.as_bytes());
        let manifest = Manifest {
            schema_version: crt_core::SCHEMA_VERSION,
            release: ReleaseHeader {
                product: "ceph".to_owned(),
                namespace: key.namespace.clone(),
                channel: key.channel.clone(),
                name: "ces-v18.2.0".to_owned(),
                base_ref: "v18.2.0".to_owned(),
                created: "2026-06-22T00:00:00+00:00".to_owned(),
                author: test_author(),
            },
            entries: vec![
                entry(
                    1,
                    "security",
                    "Fixes a CVE.",
                    Some("Default TLS is now required."),
                ),
                entry(2, "fix", "Fixes a crash.", None),
            ],
            known_issues: vec![KnownIssue {
                summary: "RGW may log spuriously.".to_owned(),
                refs: vec![],
            }],
            upgrade_notes: Some("Restart OSDs after upgrade.".to_owned()),
            branding: crt_core::Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "Hardened Ceph.".to_owned(),
                footer: "(c) Clyso".to_owned(),
            },
            render: RenderSpec {
                minijinja_version: crt_core::RENDER_MINIJINJA_VERSION.to_owned(),
                template_digest,
            },
        };
        let digest = crt_core::digest(&manifest).unwrap();
        let record = crt_core::ReleaseRecord {
            manifest,
            digest,
            signature: crt_core::ArmoredSignature("-----BEGIN PGP SIGNATURE-----".to_owned()),
        };
        store
            .put_template(&template_digest, DEFAULT_NOTES_TEMPLATE.as_bytes())
            .await
            .unwrap();
        store.put_release(&key, &record).await.unwrap();

        let notes = render_sealed_notes(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap();

        // Groups are ordered by category (`fix` precedes `security`).
        let fix_at = notes.find("## Fix").expect("fix group");
        let sec_at = notes.find("## Security").expect("security group");
        assert!(fix_at < sec_at, "groups are ordered by category");
        assert!(notes.contains("Fixes a crash."));
        assert!(notes.contains("Fixes a CVE."));
        assert!(notes.contains("Behavior change: Default TLS is now required."));
        assert!(notes.contains("## Known issues"));
        assert!(notes.contains("RGW may log spuriously."));
        assert!(notes.contains("## Upgrade notes"));
        assert!(notes.contains("Restart OSDs after upgrade."));
        assert!(notes.contains("(c) Clyso"));
        // The internal note never leaks, even with rich content.
        assert!(!notes.contains("DO-NOT-LEAK-internal"));
    }

    #[tokio::test]
    async fn render_sealed_notes_refuses_a_minijinja_version_mismatch() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let key = cfg.resolve_release_key("ces-v18.2.0").unwrap();
        // Hand-seal a record pinning a minijinja version this build does not
        // link. `render_sealed_notes` reads + version-gates + renders (it does
        // not verify the signature), so a placeholder signature suffices.
        let template_digest = Sha256::of(DEFAULT_NOTES_TEMPLATE.as_bytes());
        let manifest = Manifest {
            schema_version: crt_core::SCHEMA_VERSION,
            release: ReleaseHeader {
                product: "ceph".to_owned(),
                namespace: key.namespace.clone(),
                channel: key.channel.clone(),
                name: "ces-v18.2.0".to_owned(),
                base_ref: "v18.2.0".to_owned(),
                created: "2026-06-22T00:00:00+00:00".to_owned(),
                author: test_author(),
            },
            entries: vec![],
            known_issues: vec![],
            upgrade_notes: None,
            branding: crt_core::Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "b".to_owned(),
                footer: "f".to_owned(),
            },
            render: RenderSpec {
                minijinja_version: "0.0.0-not-linked".to_owned(),
                template_digest,
            },
        };
        let digest = crt_core::digest(&manifest).unwrap();
        let record = crt_core::ReleaseRecord {
            manifest,
            digest,
            signature: crt_core::ArmoredSignature("-----BEGIN PGP SIGNATURE-----".to_owned()),
        };
        store
            .put_template(&template_digest, DEFAULT_NOTES_TEMPLATE.as_bytes())
            .await
            .unwrap();
        store.put_release(&key, &record).await.unwrap();

        let err = render_sealed_notes(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("version mismatch"));
    }

    #[tokio::test]
    async fn materialize_writes_both_artifacts_deterministically() {
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        draft_with_one_entry(&store, &cfg, "ces-v18.2.0").await;
        let (secret, _) = test_keypair();
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

        let dir = tempfile::tempdir().unwrap();
        let (_, notes_bytes, sbom_bytes) = prepare_artifacts(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap();
        let out = write_loose_artifacts(dir.path(), &notes_bytes, &sbom_bytes)
            .await
            .unwrap();
        assert!(out.notes.ends_with("RELEASE-NOTES.md"));
        assert!(out.sbom.ends_with("sbom.cdx.json"));

        let notes = std::fs::read_to_string(&out.notes).unwrap();
        let sbom = std::fs::read_to_string(&out.sbom).unwrap();
        // The notes file equals the standalone `release notes` projection.
        assert_eq!(
            notes,
            render_sealed_notes(&store, &cfg, "ces-v18.2.0")
                .await
                .unwrap()
        );
        // The SBOM is CycloneDX and carries the backported patch.
        assert!(sbom.contains("\"bomFormat\": \"CycloneDX\""));
        assert!(sbom.contains("\"specVersion\": \"1.6\""));
        assert!(sbom.contains("backport"));

        // Re-projecting into a fresh dir yields byte-identical artifacts (the
        // leg-4 determinism contract; this is the exact path `materialize` uses).
        let dir2 = tempfile::tempdir().unwrap();
        let (_, notes2, sbom2) = prepare_artifacts(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap();
        let out2 = write_loose_artifacts(dir2.path(), &notes2, &sbom2)
            .await
            .unwrap();
        assert_eq!(sbom, std::fs::read_to_string(&out2.sbom).unwrap());
        assert_eq!(notes, std::fs::read_to_string(&out2.notes).unwrap());
    }

    #[tokio::test]
    async fn materialize_overwrites_in_place_deterministically() {
        // The fixed filenames mean a second materialize into the *same* dir
        // overwrites the first — exercise that path (`create_dir_all` on an
        // existing dir + `write` over existing files) and assert the bytes are
        // identical, since both runs project the same sealed manifest.
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        draft_with_one_entry(&store, &cfg, "ces-v18.2.0").await;
        let (secret, _) = test_keypair();
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

        let dir = tempfile::tempdir().unwrap();
        let (_, notes_a, sbom_a) = prepare_artifacts(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap();
        let first = write_loose_artifacts(dir.path(), &notes_a, &sbom_a)
            .await
            .unwrap();
        let notes1 = std::fs::read_to_string(&first.notes).unwrap();
        let sbom1 = std::fs::read_to_string(&first.sbom).unwrap();

        // Same (now non-empty) dir: must overwrite cleanly with identical bytes.
        let (_, notes_b, sbom_b) = prepare_artifacts(&store, &cfg, "ces-v18.2.0")
            .await
            .unwrap();
        let second = write_loose_artifacts(dir.path(), &notes_b, &sbom_b)
            .await
            .unwrap();
        assert_eq!(notes1, std::fs::read_to_string(&second.notes).unwrap());
        assert_eq!(sbom1, std::fs::read_to_string(&second.sbom).unwrap());
    }

    /// A fresh destination repo with a configured identity and one base commit
    /// on `main`.
    fn git_init_base(repo: &Path) {
        let g = |args: &[&str]| crate::git::git(repo, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "Test Releaser"]);
        g(&["config", "user.email", "rel@example.com"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        g(&["add", "README.md"]);
        g(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "base: initial commit",
        ]);
    }

    /// Commit `content` to `file` on `main`, capture the patch via
    /// `format-patch`, then roll `main` back so the change lives only in the
    /// returned mailbox bytes (exactly as an imported blob would).
    fn make_patch(repo: &Path, file: &str, content: &str, subject: &str) -> Vec<u8> {
        let g = |args: &[&str]| crate::git::git(repo, args).unwrap();
        std::fs::write(repo.join(file), content).unwrap();
        g(&["add", file]);
        g(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", subject]);
        let bytes = crate::git::git_bytes(repo, &["format-patch", "-1", "--stdout"]).unwrap();
        g(&["reset", "--hard", "HEAD~1"]);
        bytes
    }

    #[tokio::test]
    async fn materialize_builds_a_signed_bundle_that_verifies_offline() {
        // The M4 seam: author → seal → materialize must build a branch + signed
        // `000-RELEASE/` bundle + annotated tag whose extracted tree passes the
        // offline `verify --tree`, with no classification leaking into the
        // public bundle.
        let repo = tempfile::tempdir().unwrap();
        git_init_base(repo.path());
        let p1 = make_patch(repo.path(), "a.txt", "alpha\n", "feat: add a.txt");
        let p2 = make_patch(repo.path(), "b.txt", "beta\n", "feat: add b.txt");

        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();

        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, &p1).await;
        let h2 = imported_meta(&store, &p2).await;
        // One entry is Private — its classification must not reach the bundle.
        let mut private_fields = fields("Adds a.txt.");
        private_fields.visibility = Visibility::Private;
        private_fields.internal = Some("DO-NOT-LEAK-internal".to_owned());
        add_entries(&store, &cfg, "ces-v18.2.0", &[h1.to_hex()], &private_fields)
            .await
            .unwrap();
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h2.to_hex()],
            &fields("Adds b.txt."),
        )
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

        let summary = materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(repo.path()),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(summary.branch, "release/ces-v18.2.0");
        assert_eq!(summary.commits.len(), 2);
        assert_eq!(summary.tag, "ces-v18.2.0");

        // The annotated tag exists and carries the sealed manifest digest.
        let record = store
            .get_release(&cfg.resolve_release_key("ces-v18.2.0").unwrap())
            .await
            .unwrap();
        let g = |args: &[&str]| crate::git::git(repo.path(), args).unwrap();
        assert_eq!(g(&["cat-file", "-t", "ces-v18.2.0"]).trim(), "tag");
        assert!(
            g(&["tag", "-l", "--format=%(contents)", "ces-v18.2.0"])
                .contains(&record.digest.to_hex()),
            "the annotated tag carries the manifest digest"
        );

        // Check the tag out into a fresh tree and verify it fully offline.
        let verify_root = tempfile::tempdir().unwrap();
        let tree = verify_root.path().join("t");
        g(&[
            "-c",
            "core.autocrlf=false",
            "worktree",
            "add",
            "--detach",
            tree.to_str().unwrap(),
            "ces-v18.2.0",
        ]);
        let tree_for_verify = tree.clone();
        let verdict = tokio::task::spawn_blocking(move || {
            crate::verify::verify_tree(&tree_for_verify, &public)
        })
        .await
        .unwrap()
        .unwrap();
        match verdict {
            crate::verify::VerifyVerdict::Pass(_) => {}
            crate::verify::VerifyVerdict::SignatureFailed(r) => {
                panic!(
                    "verify --tree signature failed:\n{}",
                    crate::verify::render_report(&r)
                )
            }
            crate::verify::VerifyVerdict::VerifyFailed(r) => {
                panic!(
                    "verify --tree failed:\n{}",
                    crate::verify::render_report(&r)
                )
            }
        }

        // Public-safe by construction: the structured record + provenance name
        // no classification field, and the Private entry's internal note leaks
        // into *no* bundle file (the canary is checked across every file, so
        // RELEASE-NOTES.md and sbom.cdx.json are real backstops too).
        let bundle = tree.join(crate::verify::BUNDLE_DIR);
        let record_json = std::fs::read_to_string(bundle.join("record.json")).unwrap();
        let provenance = std::fs::read_to_string(bundle.join("provenance.json")).unwrap();
        for hay in [&record_json, &provenance] {
            assert!(!hay.contains("visibility"), "{hay}");
            assert!(!hay.contains("internal"), "{hay}");
        }
        for entry in std::fs::read_dir(&bundle).unwrap() {
            let path = entry.unwrap().path();
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(
                !body.contains("DO-NOT-LEAK-internal"),
                "internal note leaked into {}",
                path.display()
            );
        }
        // The record's BOM anchors each patch to a materialized commit.
        assert!(record_json.contains("git_commit"));
        // `.gitattributes` keeps the signed files byte-exact (slash-free pattern).
        assert_eq!(
            std::fs::read_to_string(bundle.join(".gitattributes")).unwrap(),
            "* -text\n"
        );
    }

    #[tokio::test]
    async fn materialize_push_publishes_the_branch_and_tag() {
        // `--push` is opt-in; exercise it against a *local bare* remote (no
        // network) so the real push path runs in CI. The branch + annotated tag
        // must land in the remote.
        let bare = tempfile::tempdir().unwrap();
        crate::git::git(bare.path(), &["init", "-q", "--bare"]).unwrap();

        let repo = tempfile::tempdir().unwrap();
        git_init_base(repo.path());
        crate::git::git(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        )
        .unwrap();
        let p1 = make_patch(repo.path(), "a.txt", "alpha\n", "feat: add a.txt");

        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, _) = test_keypair();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, &p1).await;
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h1.to_hex()],
            &fields("Adds a.txt."),
        )
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

        materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(repo.path()),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            true, // push
            None,
        )
        .await
        .unwrap();

        // The branch and the annotated tag are now in the bare remote.
        assert!(
            crate::git::git(
                bare.path(),
                &["rev-parse", "--verify", "release/ces-v18.2.0"]
            )
            .is_ok(),
            "the release branch was not pushed to the remote"
        );
        assert_eq!(
            crate::git::git(bare.path(), &["cat-file", "-t", "ces-v18.2.0"])
                .unwrap()
                .trim(),
            "tag",
            "the annotated tag was not pushed to the remote"
        );
    }

    #[tokio::test]
    async fn materialize_refuses_a_preexisting_tag() {
        // The tag is write-once: if it already exists in the destination repo,
        // materialize must refuse up front (before building anything), so a
        // failed run can never delete a tag it did not create.
        let repo = tempfile::tempdir().unwrap();
        git_init_base(repo.path());
        // Pre-create the release tag (on the base commit) — but no branch.
        crate::git::git(repo.path(), &["tag", "ces-v18.2.0"]).unwrap();
        let p1 = make_patch(repo.path(), "a.txt", "alpha\n", "feat: add a.txt");

        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, _) = test_keypair();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, &p1).await;
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h1.to_hex()],
            &fields("Adds a.txt."),
        )
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

        let err = materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(repo.path()),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            false,
            None,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("already exists"),
            "expected a write-once tag refusal, got: {err:#}"
        );
        // The pre-existing tag is untouched, and no release branch was created.
        assert!(crate::git::git(repo.path(), &["rev-parse", "--verify", "ces-v18.2.0"]).is_ok());
        assert!(
            crate::git::git(
                repo.path(),
                &["rev-parse", "--verify", "release/ces-v18.2.0"]
            )
            .is_err()
        );
    }

    /// Import a patch with its **real** `git patch-id --stable` (as `patch
    /// import` would compute), so the sealed entry's `patch_id` matches what
    /// leg 3 recomputes from the materialized commit. (The default
    /// `imported_meta` stores a synthetic id, which leg 3 would reject.)
    async fn import_with_real_patch_id(
        store: &ObjectBackedStore,
        repo: &Path,
        patch: &[u8],
    ) -> Sha256 {
        let hash = blob_hash(patch);
        let out = crate::git::git_with_stdin(repo, &["patch-id", "--stable"], patch).unwrap();
        let patch_id = out.split_whitespace().next().unwrap().to_owned();
        let meta = PatchMeta {
            blob_hash: hash,
            patch_id,
            author: test_author(),
            authored: "2026-06-25T00:00:00+00:00".to_owned(),
            subject: "feat: change".to_owned(),
            body: "body".to_owned(),
            cherry_picked_from: vec![],
            provenance: Provenance::UpstreamPr {
                prs: vec![],
                commits: vec![],
                state: UpstreamPrState::MergedMain,
            },
            source_repo: "ceph/ceph".to_owned(),
        };
        store.put_blob(&hash, patch).await.unwrap();
        store.put_meta(&hash, &meta).await.unwrap();
        hash
    }

    /// The `@@` hunk headers of a `format-patch` mailbox (the line-number
    /// coordinates), for proving an apply landed at an offset.
    fn hunk_headers(mailbox: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(mailbox)
            .lines()
            .filter(|l| l.starts_with("@@"))
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn verify_release_runs_ref_legs_on_a_materialized_release() {
        // With --repo at a materialized release, the ref-conditional legs run and
        // pass: bundle signature, in-tree record faithfulness, git anchoring, and
        // artifact faithfulness.
        let repo = tempfile::tempdir().unwrap();
        git_init_base(repo.path());
        let p1 = make_patch(repo.path(), "a.txt", "alpha\n", "feat: add a.txt");

        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = import_with_real_patch_id(&store, repo.path(), &p1).await;
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h1.to_hex()],
            &fields("Adds a.txt."),
        )
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
        materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(repo.path()),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            false,
            None,
        )
        .await
        .unwrap();

        let verdict =
            crate::verify::verify_release(&store, &cfg, "ces-v18.2.0", &public, Some(repo.path()))
                .await
                .unwrap();
        let report = match verdict {
            crate::verify::VerifyVerdict::Pass(r) => r,
            other => panic!("expected Pass, got: {}", render_verdict(&other)),
        };
        let rendered = crate::verify::render_report(&report);
        for needle in [
            "[pass] leg 0 signature (000-RELEASE bundle)",
            "[pass] leg 2 cross-reference (in-tree record)",
            "[pass] leg 3 git anchoring",
            "[pass] leg 4 artifact faithfulness",
        ] {
            assert!(
                rendered.contains(needle),
                "missing {needle:?} in:\n{rendered}"
            );
        }
    }

    #[tokio::test]
    async fn verify_leg3_anchors_through_an_apply_offset() {
        // The reason leg 3 recomputes patch_id from the *commit diff* (not the
        // stored blob): offset-invariance. Build a patch, then shift the base so
        // it applies at an offset — the materialized commit's hunk coordinates
        // differ from the stored mailbox, yet `patch-id --stable` still anchors.
        let repo = tempfile::tempdir().unwrap();
        let p = repo.path();
        let g = |args: &[&str]| crate::git::git(p, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "T"]);
        g(&["config", "user.email", "t@example.com"]);
        // A file long enough that the change sits in the *middle*: its hunk
        // context (±3 lines) never touches the file edges, so prepending lines
        // shifts the hunk without breaking `git apply`'s exact-context match.
        let head: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        let tail: String = (11..=20).map(|i| format!("line {i}\n")).collect();
        let base_v0 = format!("{head}TARGET\n{tail}");
        let patched = format!("{head}CHANGED\n{tail}");
        let offset_base = format!("PRE1\nPRE2\nPRE3\n{base_v0}");

        std::fs::write(p.join("f.txt"), &base_v0).unwrap();
        g(&["add", "f.txt"]);
        g(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "base v0",
        ]);
        // The patch (TARGET -> CHANGED), generated against base v0.
        std::fs::write(p.join("f.txt"), &patched).unwrap();
        g(&["add", "f.txt"]);
        g(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "feat: change TARGET",
        ]);
        let patch = crate::git::git_bytes(p, &["format-patch", "-1", "--stdout"]).unwrap();
        g(&["reset", "--hard", "HEAD~1"]);
        // Shift the base: prepend lines far above the hunk so the patch applies
        // at an offset (its context still matches, just lower in the file).
        std::fs::write(p.join("f.txt"), &offset_base).unwrap();
        g(&["add", "f.txt"]);
        g(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "base: prepend lines (offsets the patch)",
        ]);

        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h = import_with_real_patch_id(&store, p, &patch).await;
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h.to_hex()],
            &fields("Change TARGET."),
        )
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
        materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(p),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            false,
            None,
        )
        .await
        .unwrap();

        // Canary: the materialized commit's hunk coordinates differ from the
        // stored patch (the apply landed at an offset) — so leg 3 is not
        // tautological — yet the offset-invariant patch-id is identical.
        let patch_commit = g(&["rev-parse", "ces-v18.2.0^"]).trim().to_owned();
        let commit_diff =
            crate::git::git_bytes(p, &["format-patch", "-1", "--stdout", &patch_commit]).unwrap();
        assert_ne!(
            hunk_headers(&commit_diff),
            hunk_headers(&patch),
            "the patch must apply at a different offset, else leg 3 proves nothing"
        );
        let pid = |m: &[u8]| {
            crate::git::git_with_stdin(p, &["patch-id", "--stable"], m)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            pid(&commit_diff),
            pid(&patch),
            "patch-id --stable is offset-invariant"
        );

        // Leg 3 anchors despite the offset.
        let verdict = crate::verify::verify_release(&store, &cfg, "ces-v18.2.0", &public, Some(p))
            .await
            .unwrap();
        let report = match verdict {
            crate::verify::VerifyVerdict::Pass(r) => r,
            other => panic!("expected Pass, got: {}", render_verdict(&other)),
        };
        assert!(
            crate::verify::render_report(&report).contains("[pass] leg 3 git anchoring"),
            "leg 3 must anchor through the offset"
        );
    }

    #[tokio::test]
    async fn verify_leg3_fails_on_a_mismatched_patch_id() {
        // A sealed entry whose patch_id does not match what `git am` landed must
        // fail leg 3. `imported_meta` stores a synthetic patch_id, so the
        // recomputed (real) id will not match.
        let repo = tempfile::tempdir().unwrap();
        git_init_base(repo.path());
        let p1 = make_patch(repo.path(), "a.txt", "alpha\n", "feat: add a.txt");

        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = imported_meta(&store, &p1).await; // synthetic patch_id
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h1.to_hex()],
            &fields("Adds a.txt."),
        )
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
        materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(repo.path()),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            false,
            None,
        )
        .await
        .unwrap();

        let verdict =
            crate::verify::verify_release(&store, &cfg, "ces-v18.2.0", &public, Some(repo.path()))
                .await
                .unwrap();
        let report = match verdict {
            crate::verify::VerifyVerdict::VerifyFailed(r) => r,
            other => panic!("expected VerifyFailed, got: {}", render_verdict(&other)),
        };
        assert!(
            crate::verify::render_report(&report).contains("[FAIL] leg 3 git anchoring"),
            "leg 3 must reject a mismatched patch_id"
        );
    }

    /// Render any verdict (with its report) for test panics.
    fn render_verdict(v: &crate::verify::VerifyVerdict) -> String {
        let (tag, r) = match v {
            crate::verify::VerifyVerdict::Pass(r) => ("Pass", r),
            crate::verify::VerifyVerdict::SignatureFailed(r) => ("SignatureFailed", r),
            crate::verify::VerifyVerdict::VerifyFailed(r) => ("VerifyFailed", r),
        };
        format!("{tag}\n{}", crate::verify::render_report(r))
    }

    /// Seal a one-patch release (real patch_id) and materialize it into a fresh
    /// synthetic repo. Returns the store, config, repo (kept alive), and the
    /// signing key pair — the base for the forge-then-resign failure tests.
    async fn materialize_one_patch_release()
    -> (ObjectBackedStore, Config, tempfile::TempDir, String, String) {
        let repo = tempfile::tempdir().unwrap();
        git_init_base(repo.path());
        let p1 = make_patch(repo.path(), "a.txt", "alpha\n", "feat: add a.txt");
        let store = ObjectBackedStore::in_memory();
        let cfg = test_config();
        let (secret, public) = test_keypair();
        new_release(
            &store,
            &cfg,
            "ces-v18.2.0",
            "main",
            test_author(),
            "2026-06-25T00:00:00+00:00".to_owned(),
        )
        .await
        .unwrap();
        let h1 = import_with_real_patch_id(&store, repo.path(), &p1).await;
        add_entries(
            &store,
            &cfg,
            "ces-v18.2.0",
            &[h1.to_hex()],
            &fields("Adds a.txt."),
        )
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
        materialize(
            &store,
            &cfg,
            "ces-v18.2.0",
            Some(repo.path()),
            None,
            &secret,
            None,
            "2026-06-25T12:00:00+00:00".to_owned(),
            false,
            None,
        )
        .await
        .unwrap();
        (store, cfg, repo, secret, public)
    }

    /// Re-forge the materialized bundle at `tag` into a **validly re-signed** but
    /// possibly unfaithful one: check it out, let `mutate` rewrite 000-RELEASE/
    /// sidecar files and return the new `record.json` bytes, re-sign those with
    /// `secret`, amend the bundle commit, and move the tag onto it. A normal
    /// materialize can never produce these states — this forges the
    /// internally-consistent-but-unfaithful bundle that legs 2b / 4 must catch.
    fn reforge_bundle(repo: &Path, tag: &str, secret: &str, mutate: impl FnOnce(&Path) -> Vec<u8>) {
        let scratch = tempfile::tempdir().unwrap();
        let work = scratch.path().join("t");
        let work_str = work.to_str().unwrap();
        crate::git::git(
            repo,
            &[
                "-c",
                "core.autocrlf=false",
                "worktree",
                "add",
                "--detach",
                work_str,
                tag,
            ],
        )
        .unwrap();
        let bundle = work.join("000-RELEASE");
        let record_bytes = mutate(&bundle);
        let sig = crt_core::sign_manifest(rand::thread_rng(), &record_bytes, secret, None).unwrap();
        std::fs::write(bundle.join("record.json"), &record_bytes).unwrap();
        std::fs::write(bundle.join("record.json.asc"), &sig.0).unwrap();
        crate::git::git(&work, &["add", "000-RELEASE"]).unwrap();
        crate::git::git(
            &work,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--amend",
                "--no-edit",
            ],
        )
        .unwrap();
        crate::git::git(
            &work,
            &[
                "-c",
                "tag.gpgsign=false",
                "tag",
                "-f",
                "-a",
                tag,
                "-m",
                "reforged",
            ],
        )
        .unwrap();
        crate::git::git(repo, &["worktree", "remove", "--force", work_str]).unwrap();
    }

    #[tokio::test]
    async fn verify_leg2_fails_on_a_forged_in_tree_record() {
        // A validly re-signed record.json whose s3_manifest_digest no longer
        // points at the sealed manifest. The bundle signature (leg 0), schema
        // (leg 1), source/bundle digests (leg 2), and git anchoring (leg 3) all
        // still pass — only the in-tree cross-reference (leg 2b) catches it.
        let (store, cfg, repo, secret, public) = materialize_one_patch_release().await;
        reforge_bundle(repo.path(), "ces-v18.2.0", &secret, |bundle| {
            let bytes = std::fs::read(bundle.join("record.json")).unwrap();
            let mut record: crt_core::MaterializationRecord =
                serde_json::from_slice(&bytes).unwrap();
            record.s3_manifest_digest = blob_hash(b"not the sealed manifest digest");
            serde_json::to_vec_pretty(&record).unwrap()
        });

        let verdict =
            crate::verify::verify_release(&store, &cfg, "ces-v18.2.0", &public, Some(repo.path()))
                .await
                .unwrap();
        let report = match verdict {
            crate::verify::VerifyVerdict::VerifyFailed(r) => r,
            other => panic!("expected VerifyFailed, got: {}", render_verdict(&other)),
        };
        let rendered = crate::verify::render_report(&report);
        assert!(
            rendered.contains("[FAIL] leg 2 cross-reference (in-tree record)"),
            "{rendered}"
        );
    }

    #[tokio::test]
    async fn verify_leg4_fails_on_a_self_consistent_but_unfaithful_sbom() {
        // A tampered SBOM whose digest is updated in the re-signed record, so
        // legs 0/1/2/2b/3 all pass — only leg 4's byte-compare against a
        // re-derivation from the sealed manifest catches it.
        let (store, cfg, repo, secret, public) = materialize_one_patch_release().await;
        reforge_bundle(repo.path(), "ces-v18.2.0", &secret, |bundle| {
            let tampered = b"{\"bomFormat\": \"CycloneDX\", \"tampered\": true}\n".to_vec();
            std::fs::write(bundle.join("sbom.cdx.json"), &tampered).unwrap();
            let bytes = std::fs::read(bundle.join("record.json")).unwrap();
            let mut record: crt_core::MaterializationRecord =
                serde_json::from_slice(&bytes).unwrap();
            record
                .bundle_digests
                .insert("sbom.cdx.json".to_owned(), crt_core::Sha256::of(&tampered));
            serde_json::to_vec_pretty(&record).unwrap()
        });

        let verdict =
            crate::verify::verify_release(&store, &cfg, "ces-v18.2.0", &public, Some(repo.path()))
                .await
                .unwrap();
        let report = match verdict {
            crate::verify::VerifyVerdict::VerifyFailed(r) => r,
            other => panic!("expected VerifyFailed, got: {}", render_verdict(&other)),
        };
        let rendered = crate::verify::render_report(&report);
        assert!(
            rendered.contains("[FAIL] leg 4 artifact faithfulness"),
            "{rendered}"
        );
    }

    #[test]
    fn parses_an_editor_buffer() {
        let buf = "\
# a comment
@@ public-summary (required)
Fixes a serious bug.

@@ behavior-change (optional)
The default changed.
@@ upgrade-notes (optional)
";
        let (summary, behavior, upgrade) = parse_editor_buffer(buf).unwrap();
        assert_eq!(summary, "Fixes a serious bug.");
        assert_eq!(behavior.as_deref(), Some("The default changed."));
        assert_eq!(upgrade, None);
    }

    #[test]
    fn editor_buffer_requires_a_public_summary() {
        let buf = "@@ public-summary (required)\n\n@@ behavior-change\nx\n";
        assert!(parse_editor_buffer(buf).is_err());
    }
}
