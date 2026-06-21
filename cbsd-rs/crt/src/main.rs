// crt — Ceph Release Tool v2 (CLI).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! `crt` ingests, seals, and materializes downstream Ceph releases. M1
//! delivers patch ingestion into a content-addressed store (design §3–§5,
//! plan M1).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crt_store::{ObjectBackedStore, S3Settings};

mod config;
mod git;
mod import;
mod secrets;

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
    }
    Ok(())
}
