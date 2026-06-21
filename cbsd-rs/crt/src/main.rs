// crt — Ceph Release Tool v2 (CLI).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! `crt` ingests, seals, and materializes downstream Ceph releases. M1
//! delivers patch ingestion into a content-addressed store (design §3–§5,
//! plan M1).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crt_store::ObjectBackedStore;

mod config;
mod git;
mod import;

#[derive(Parser)]
#[command(name = "crt", version, about = "Ceph Release Tool")]
struct Cli {
    /// Path to the (git-ignored) config file.
    #[arg(long, global = true, default_value = "crt.config.yaml")]
    config: PathBuf,
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::load(&cli.config)?;

    match cli.command {
        Command::Patch { cmd } => match cmd {
            PatchCmd::Import { repo, range } => {
                let store = ObjectBackedStore::local(&cfg.store.local)?;
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
