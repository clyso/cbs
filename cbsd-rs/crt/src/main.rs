// crt — Ceph Release Tool v2 (CLI).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! `crt` ingests, seals, and materializes downstream Ceph releases. M1
//! delivers patch ingestion into a content-addressed store (design §3–§5,
//! plan M1).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use crt_store::{ObjectBackedStore, S3Settings, Store};

use crate::release::{
    BlastArg, ConflictArg, CoverageArg, EntryFields, JustificationArg, VisibilityArg,
};

mod config;
mod git;
mod import;
mod release;
mod secrets;
mod vault;

#[derive(Parser)]
#[command(name = "crt", version, about = "Ceph Release Tool")]
struct Cli {
    /// Path to the (git-ignored) config file.
    #[arg(
        long,
        global = true,
        env = "CRT_CONFIG",
        default_value = "crt.config.yaml"
    )]
    config: PathBuf,
    /// Path to the (git-ignored) secrets file.
    #[arg(
        long,
        global = true,
        env = "CRT_SECRETS",
        default_value = "crt.secrets.yaml"
    )]
    secrets: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Patch operations.
    Patch {
        #[command(subcommand)]
        cmd: PatchCmd,
    },
    /// Release authoring.
    Release {
        #[command(subcommand)]
        cmd: ReleaseCmd,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Create a new, empty draft release. The name resolves to a channel by
    /// prefix (e.g. `ces-v18.2.0` → channel `ces`).
    New {
        /// Release name (resolves to a configured channel).
        name: String,
        /// Upstream base ref the patches apply on top of (e.g. `v18.2.0`).
        #[arg(long)]
        base_ref: String,
        /// Release author name (defaults to `git config user.name`).
        #[arg(long)]
        author_name: Option<String>,
        /// Release author email (defaults to `git config user.email`).
        #[arg(long)]
        author_email: Option<String>,
    },
    /// Add one or more imported patch blobs to a draft as entries. The metadata
    /// flags apply to every blob listed.
    Add {
        /// Release name.
        name: String,
        /// Blob hashes (full 64-char hex) of already-imported patches.
        #[arg(required = true)]
        blob_hash: Vec<String>,
        /// Risk subsystem label (validated against `risk_components`).
        #[arg(long)]
        component: String,
        #[arg(long, value_enum)]
        blast: BlastArg,
        #[arg(long, value_enum)]
        conflict: ConflictArg,
        #[arg(long, value_enum)]
        coverage: CoverageArg,
        /// Notes grouping (e.g. `security`/`feature`/`fix`/`integration`).
        #[arg(long)]
        category: String,
        /// Per-patch visibility (recorded but inert in the MVP).
        #[arg(long, value_enum, default_value = "public")]
        visibility: VisibilityArg,
        /// Why the patch is carried.
        #[arg(long, value_enum)]
        justification: JustificationArg,
        /// Tracker/PR/CVE reference (repeatable).
        #[arg(long = "ref")]
        refs: Vec<String>,
        /// Internal-only note (stored, never rendered or materialized).
        #[arg(long)]
        internal: Option<String>,
        /// Public summary rendered into the notes. If omitted, `$EDITOR` opens
        /// to compose the public summary / behavior change / upgrade notes.
        #[arg(long)]
        public_summary: Option<String>,
        /// Per-entry behavior-change note.
        #[arg(long)]
        behavior_change: Option<String>,
        /// Per-entry upgrade note.
        #[arg(long)]
        upgrade_notes: Option<String>,
    },
    /// Seal a draft into a signed, write-once release. Fetches the signing key
    /// from Vault, signs the canonical manifest, writes the release record, and
    /// removes the draft.
    Seal {
        /// Release name.
        name: String,
    },
    /// List the sealed releases in the store.
    List,
    /// Show a draft (or, if none, the sealed release) for a name.
    Info {
        /// Release name.
        name: String,
    },
}

#[derive(Subcommand)]
enum PatchCmd {
    /// Import patches into the content-addressed store, from a local git range
    /// (`--range`) or a GitHub PR (`--pr`).
    Import {
        /// Path to the local git repository (the PR's head/base are fetched
        /// into it; patch bytes always come from a local `git format-patch`).
        #[arg(long)]
        repo: PathBuf,
        /// Commit range, e.g. `A..B`.
        #[arg(long, conflicts_with = "pr", required_unless_present = "pr")]
        range: Option<String>,
        /// GitHub PR URL, e.g. `https://github.com/ceph/ceph/pull/12345`.
        #[arg(long)]
        pr: Option<String>,
        /// GitHub token for the API (raises rate limits). The git fetch is
        /// anonymous, so private-repo PRs are unsupported.
        #[arg(long, env = "GITHUB_TOKEN")]
        github_token: Option<String>,
    },
}

/// Build the configured store backend. The S3 backend reads credentials from
/// the secrets file; the local backend needs no secrets.
fn open_store(store: &config::StoreConfig, secrets_path: &Path) -> Result<ObjectBackedStore> {
    match store {
        config::StoreConfig::Local(path) => Ok(ObjectBackedStore::local(path)?),
        config::StoreConfig::S3(s3) => {
            let creds = secrets::load(secrets_path)?.s3.with_context(|| {
                format!(
                    "config selects an S3 store but {} has no `s3` section",
                    secrets_path.display()
                )
            })?;
            Ok(ObjectBackedStore::s3(&S3Settings {
                endpoint: s3.endpoint.clone(),
                region: s3.region.clone(),
                bucket: s3.bucket.clone(),
                prefix: s3.prefix.clone(),
                access_key_id: creds.access_key_id,
                secret_access_key: creds.secret_access_key,
            })?)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Pin `ring` as the process-wide rustls crypto provider before any TLS
    // client is built (octocrab for `patch import --pr`, reqwest for `release
    // verify`, object_store for S3). With aws-lc-rs dropped `ring` is the sole
    // provider, but pinning is explicit and guards against a future dependency
    // re-introducing a second provider — which makes rustls 0.23 refuse to
    // auto-select and panic at the first handshake.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("the rustls crypto provider is installed once at startup");

    let cli = Cli::parse();
    let cfg = config::load(&cli.config)?;

    match cli.command {
        Command::Patch { cmd } => match cmd {
            PatchCmd::Import {
                repo,
                range,
                pr,
                github_token,
            } => {
                let store = open_store(&cfg.store, &cli.secrets)?;
                let imported = if let Some(pr) = pr {
                    import::import_pr(&store, &repo, &pr, github_token.as_deref()).await?
                } else {
                    // clap guarantees exactly one of `--range` / `--pr`.
                    let range = range.expect("clap requires --range when --pr is absent");
                    let source = repo.display().to_string();
                    import::import_range(&store, &repo, &range, &source).await?
                };
                for p in &imported {
                    let tag = if p.already_present {
                        "present "
                    } else {
                        "imported"
                    };
                    println!("{tag} {} {}", p.blob_hash, p.subject);
                    if let Some(eq) = &p.equivalent_to {
                        eprintln!(
                            "  warning: equivalent to existing patch {eq} \
                             (same patch_id, different bytes) — consider reusing it"
                        );
                    }
                }
                eprintln!(
                    "{} patch(es) processed for {}",
                    imported.len(),
                    cfg.component
                );
            }
        },
        Command::Release { cmd } => {
            let store = open_store(&cfg.store, &cli.secrets)?;
            match cmd {
                ReleaseCmd::New {
                    name,
                    base_ref,
                    author_name,
                    author_email,
                } => {
                    let author = release::resolve_author(author_name, author_email)?;
                    let created = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false);
                    let key = release::new_release(&store, &cfg, &name, &base_ref, author, created)
                        .await?;
                    println!(
                        "created draft {} ({}/{}) base {base_ref}",
                        key.name, key.namespace, key.channel
                    );
                }
                ReleaseCmd::Add {
                    name,
                    blob_hash,
                    component,
                    blast,
                    conflict,
                    coverage,
                    category,
                    visibility,
                    justification,
                    refs,
                    internal,
                    public_summary,
                    behavior_change,
                    upgrade_notes,
                } => {
                    // Narrative fields come from flags, or — when
                    // `--public-summary` is omitted — a single `$EDITOR` session.
                    // On the editor path, explicit `--behavior-change` /
                    // `--upgrade-notes` flags still win over the editor's
                    // sections rather than being silently discarded.
                    let (public_summary, behavior_change, upgrade_notes) = if let Some(summary) =
                        public_summary
                    {
                        (summary, behavior_change, upgrade_notes)
                    } else {
                        let (ed_summary, ed_behavior, ed_upgrade) = release::compose_via_editor()?;
                        (
                            ed_summary,
                            behavior_change.or(ed_behavior),
                            upgrade_notes.or(ed_upgrade),
                        )
                    };
                    let fields = EntryFields {
                        visibility: visibility.into(),
                        category,
                        component,
                        blast: blast.into(),
                        conflict: conflict.into(),
                        coverage: coverage.into(),
                        kind: justification.into(),
                        refs,
                        public_summary,
                        internal,
                        behavior_change,
                        upgrade_notes,
                    };
                    let result =
                        release::add_entries(&store, &cfg, &name, &blob_hash, &fields).await?;
                    for h in &result.added {
                        println!("added {h}");
                    }
                    for h in &result.skipped {
                        eprintln!("skipped {h} (already in the draft)");
                    }
                    eprintln!(
                        "{} entr(ies) added, {} skipped",
                        result.added.len(),
                        result.skipped.len()
                    );
                }
                ReleaseCmd::Seal { name } => {
                    // Refuse a re-seal *before* fetching the signing key, so an
                    // already-sealed release never pulls the private key into the
                    // process. `put_release` (inside `seal_release`) remains the
                    // authoritative write-once guard against a race.
                    let key = cfg.resolve_release_key(&name)?;
                    match store.get_release(&key).await {
                        Ok(_) => anyhow::bail!(
                            "a sealed release named {name:?} already exists \
                             (releases are write-once)"
                        ),
                        Err(e) if e.is_not_found() => {}
                        Err(e) => return Err(e.into()),
                    }
                    let secrets = secrets::load(&cli.secrets)?;
                    let vault = secrets.vault.with_context(|| {
                        format!(
                            "sealing needs a `vault` section in {}",
                            cli.secrets.display()
                        )
                    })?;
                    let signing = vault::fetch_signing_key(&vault).await?;
                    let sealed = release::seal_release(
                        &store,
                        &cfg,
                        &name,
                        &signing.armored_private_key,
                        signing.passphrase.as_deref(),
                        rand::thread_rng(),
                    )
                    .await?;
                    println!(
                        "sealed {} ({}/{})",
                        sealed.name, sealed.namespace, sealed.channel
                    );
                }
                ReleaseCmd::List => {
                    let keys = release::list_releases(&store).await?;
                    for k in &keys {
                        println!("{}/{}/{}", k.namespace, k.channel, k.name);
                    }
                    eprintln!("{} sealed release(s)", keys.len());
                }
                ReleaseCmd::Info { name } => {
                    print!("{}", release::show_info(&store, &cfg, &name).await?);
                }
            }
        }
    }
    Ok(())
}
