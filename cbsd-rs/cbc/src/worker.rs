// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Worker administration: list, register, deregister, token rotation.

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::builds::format_timestamp;
use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct WorkerArgs {
    #[command(subcommand)]
    command: WorkerCommands,
}

#[derive(Subcommand)]
enum WorkerCommands {
    /// List all registered workers with live status
    List,
    /// Register a new worker and print the worker token
    Register(RegisterArgs),
    /// Deregister a worker and revoke its API key
    Deregister(DeregisterArgs),
    /// Rotate a worker's API key and print the new token
    RegenerateToken(RegenerateTokenArgs),
}

#[derive(Args)]
struct RegisterArgs {
    /// Worker name (alphanumeric, hyphens, underscores, 1-64 chars)
    name: String,
    /// Architecture: x86_64 or aarch64
    arch: String,
}

#[derive(Args)]
struct DeregisterArgs {
    /// Worker ID (full UUID or unique prefix)
    id: String,
    /// Confirm this irreversible operation
    #[arg(long = "yes-i-really-mean-it")]
    yes_i_really_mean_it: bool,
}

#[derive(Args)]
struct RegenerateTokenArgs {
    /// Worker ID (full UUID or unique prefix)
    id: String,
}

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WorkerInfo {
    worker_id: String,
    name: String,
    arch: String,
    status: String,
    #[serde(default)]
    version: Option<String>,
    last_seen: Option<i64>,
    current_build_id: Option<i64>,
}

#[derive(Deserialize)]
struct RegisterWorkerResponse {
    worker_id: String,
    name: String,
    #[allow(dead_code)]
    arch: String,
    worker_token: String,
}

#[derive(Serialize)]
struct RegisterWorkerBody {
    name: String,
    arch: String,
}

#[derive(Deserialize)]
struct DeregisterResponse {
    detail: String,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: WorkerArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    match args.command {
        WorkerCommands::List => cmd_list(config_path, debug, no_tls_verify).await,
        WorkerCommands::Register(a) => cmd_register(a, config_path, debug, no_tls_verify).await,
        WorkerCommands::Deregister(a) => cmd_deregister(a, config_path, debug, no_tls_verify).await,
        WorkerCommands::RegenerateToken(a) => {
            cmd_regenerate_token(a, config_path, debug, no_tls_verify).await
        }
    }
}

// ---------------------------------------------------------------------------
// ID prefix matching
// ---------------------------------------------------------------------------

/// Resolve a worker ID prefix to a full `(worker_id, name)` pair.
///
/// 1. Fetch the worker list via `GET /api/workers`.
/// 2. On 403 (no `workers:view`): fall back to treating `prefix` as a full
///    UUID and return `(prefix, "unknown")`.
/// 3. Filter workers whose `worker_id` starts with `prefix`.
/// 4. Zero matches: error.
/// 5. Multiple matches: error listing the ambiguous candidates.
/// 6. Exactly one: return `(id, name)`.
async fn resolve_worker_id(client: &CbcClient, prefix: &str) -> Result<(String, String), Error> {
    let workers: Vec<WorkerInfo> = match client.get("workers").await {
        Ok(list) => list,
        Err(Error::Api { status: 403, .. }) => {
            // No workers:view capability — fall back to full UUID.
            return Ok((prefix.to_string(), "unknown".to_string()));
        }
        Err(e) => return Err(e),
    };

    let matches: Vec<&WorkerInfo> = workers
        .iter()
        .filter(|w| w.worker_id.starts_with(prefix))
        .collect();

    match matches.len() {
        0 => Err(Error::Other(format!("no worker matching '{prefix}'"))),
        1 => Ok((matches[0].worker_id.clone(), matches[0].name.clone())),
        _ => {
            let list: Vec<String> = matches
                .iter()
                .map(|w| {
                    format!(
                        "  {}  {}",
                        &w.worker_id[..12.min(w.worker_id.len())],
                        w.name
                    )
                })
                .collect();
            Err(Error::Other(format!(
                "ambiguous prefix '{prefix}' matches:\n{}",
                list.join("\n"),
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// worker list
// ---------------------------------------------------------------------------

async fn cmd_list(
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let workers: Vec<WorkerInfo> = client.get("workers").await?;

    if workers.is_empty() {
        println!("no workers found");
        return Ok(());
    }

    println!(
        "  {name:<18} {arch:<10} {status:<14} {ver:<20} {build:<7} LAST SEEN",
        name = "NAME",
        arch = "ARCH",
        status = "STATUS",
        ver = "VERSION",
        build = "BUILD",
    );

    for w in &workers {
        let build = w
            .current_build_id
            .map(|id| format!("#{id}"))
            .unwrap_or_else(|| "-".to_string());

        let last_seen = w
            .last_seen
            .map(format_timestamp)
            .unwrap_or_else(|| "-".to_string());

        let ver = w.version.as_deref().unwrap_or("-");

        println!(
            "  {:<18} {:<10} {:<14} {:<20} {:<7} {}",
            w.name, w.arch, w.status, ver, build, last_seen,
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// worker register
// ---------------------------------------------------------------------------

async fn cmd_register(
    args: RegisterArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    // Client-side arch validation.
    if args.arch != "x86_64" && args.arch != "aarch64" {
        return Err(Error::Other(format!(
            "invalid architecture '{}': expected x86_64 or aarch64",
            args.arch,
        )));
    }

    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = RegisterWorkerBody {
        name: args.name.clone(),
        arch: args.arch,
    };

    let resp: RegisterWorkerResponse = client.post("admin/workers", &body).await?;

    println!("worker '{}' registered", resp.name);
    println!("  id: {}", resp.worker_id);
    println!();
    println!("worker token (save this - it cannot be recovered):");
    println!();
    println!("  {}", resp.worker_token);
    println!();
    println!("Set as CBSD_WORKER_TOKEN in the worker environment,");
    println!("or as 'worker-token' in worker.yaml.");

    Ok(())
}

// ---------------------------------------------------------------------------
// worker deregister
// ---------------------------------------------------------------------------

async fn cmd_deregister(
    args: DeregisterArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    if !args.yes_i_really_mean_it {
        eprintln!("this is a destructive operation; pass --yes-i-really-mean-it to confirm");
        return Err(Error::Other("confirmation required".into()));
    }

    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let (resolved_id, name) = resolve_worker_id(&client, &args.id).await?;

    let resp: DeregisterResponse = client
        .delete(&format!("admin/workers/{resolved_id}"))
        .await?;

    // Prefer the server's detail message, but fall back to the name from
    // prefix resolution if the server response is generic.
    if name != "unknown" {
        println!("worker '{name}' deregistered (API key revoked)");
    } else {
        println!("{}", resp.detail);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// worker regenerate-token
// ---------------------------------------------------------------------------

async fn cmd_regenerate_token(
    args: RegenerateTokenArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let (resolved_id, name) = resolve_worker_id(&client, &args.id).await?;

    let resp: RegisterWorkerResponse = client
        .post(
            &format!("admin/workers/{resolved_id}/regenerate-token"),
            &(),
        )
        .await?;

    let display_name = if name != "unknown" { &name } else { &resp.name };

    println!("worker '{display_name}' token regenerated");
    println!();
    println!("WARNING: if the worker is currently building, the");
    println!("build will be re-queued.");
    println!();
    println!("new worker token (save this - it cannot be recovered):");
    println!();
    println!("  {}", resp.worker_token);
    println!();
    println!("The worker must be restarted with the new token.");

    Ok(())
}
