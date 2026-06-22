// crt — draft release authoring (`release new` / `add` / `info`).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Author a store-backed draft release (design §3/§5, plan M2.4). A draft is
//! created by `new`, populated with patch entries by `add`, and inspected by
//! `info`. Drafts live in the shared store (not on local disk) so any operator
//! can pick up in-progress work; `seal` (M2.5) consumes one.
//!
//! Entry metadata is authored with flags; the narrative fields
//! (`public_summary` / `behavior_change` / `upgrade_notes`) come from flags or,
//! when `--public-summary` is omitted, from a single `$EDITOR` session. The
//! pure helpers (resolution lives in [`crate::config`]; entry construction,
//! editor-buffer parsing, and rendering live here) are unit-tested; the IO
//! shims (`$EDITOR`, `git config` author lookup) are thin.

use anyhow::{Context, Result, anyhow, bail};
use crt_core::{
    Blast, Conflict, Coverage, Draft, Identity, Justification, JustificationKind, KnownIssue,
    Lifecycle, ManifestEntry, PatchMeta, PatchStatus, ReleaseHeader, Risk, Sha256, Visibility,
};
use crt_store::Store;

use crate::config::Config;

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
            risk_components: vec!["rgw".to_owned()],
            namespaces,
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
