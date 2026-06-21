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
    /// Import patches from a local git range into the content-addressed store.
    Import {
        /// Path to the source git repository.
        #[arg(long)]
        repo: PathBuf,
        /// Commit range, e.g. `A..B`.
        #[arg(long)]
        range: String,
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
            PatchCmd::Import { repo, range } => {
                let store = open_store(&cfg.store, &cli.secrets)?;
                let source = repo.display().to_string();
                let imported = import::import_range(&store, &repo, &range, &source).await?;
                for p in &imported {
                    let tag = if p.already_present {
                        "present "
                    } else {
                        "imported"
                    };
                    println!("{tag} {} {}", p.blob_hash, p.subject);
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
