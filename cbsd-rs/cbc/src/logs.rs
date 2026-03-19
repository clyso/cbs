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

//! Build log commands: tail, follow (SSE), and download.

use clap::{Args, Subcommand};
use futures_util::StreamExt;
use reqwest::Method;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct LogsArgs {
    #[command(subcommand)]
    command: LogsCommands,
}

#[derive(Subcommand)]
enum LogsCommands {
    /// Show the last N lines of a build log
    Tail(TailArgs),
    /// Follow build log output in real-time (SSE)
    Follow(FollowArgs),
    /// Download the full build log to a file
    Get(GetArgs),
}

#[derive(Args)]
struct TailArgs {
    /// Build ID
    id: i64,

    /// Number of lines to show
    #[arg(short = 'n', long = "lines", default_value = "30")]
    n: u32,
}

#[derive(Args)]
struct FollowArgs {
    /// Build ID
    id: i64,
}

#[derive(Args)]
struct GetArgs {
    /// Build ID
    id: i64,

    /// Output file path (default: build-{id}.log)
    #[arg(short, long)]
    output: Option<String>,
}

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TailResponse {
    #[allow(dead_code)]
    build_id: i64,
    lines: Vec<String>,
    total_lines: u64,
    returned: u64,
}

#[derive(Deserialize)]
struct BuildStateResponse {
    #[allow(dead_code)]
    id: i64,
    state: String,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: LogsArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    match args.command {
        LogsCommands::Tail(a) => cmd_tail(a, config_path, debug).await,
        LogsCommands::Follow(a) => cmd_follow(a, config_path, debug).await,
        LogsCommands::Get(a) => cmd_get(a, config_path, debug).await,
    }
}

// ---------------------------------------------------------------------------
// build logs tail
// ---------------------------------------------------------------------------

async fn cmd_tail(
    args: TailArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let resp: TailResponse = client
        .get(&format!("builds/{}/logs/tail?n={}", args.id, args.n))
        .await?;

    println!("(showing {} of {} lines)", resp.returned, resp.total_lines);
    for line in &resp.lines {
        println!("{line}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// build logs follow
// ---------------------------------------------------------------------------

/// Maximum consecutive SSE errors before giving up.
const MAX_RETRIES: u32 = 3;

async fn cmd_follow(
    args: FollowArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let request =
        client.request_builder(Method::GET, &format!("builds/{}/logs/follow", args.id))?;

    let mut es = EventSource::new(request)
        .map_err(|e| Error::Other(format!("cannot create event source: {e}")))?;

    let mut retries: u32 = 0;

    while let Some(event) = es.next().await {
        match event {
            Ok(Event::Message(msg)) if msg.event == "done" => {
                break;
            }
            Ok(Event::Message(msg)) => {
                println!("{}", msg.data);
                retries = 0;
            }
            Ok(Event::Open) => {
                retries = 0;
            }
            Err(e) => {
                retries += 1;
                if retries >= MAX_RETRIES {
                    eprintln!("stream error: {e}");
                    es.close();
                    break;
                }
            }
        }
    }

    // Fetch the final build state.
    let build: BuildStateResponse = client.get(&format!("builds/{}", args.id)).await?;

    println!("--- build {} finished: {} ---", args.id, build.state);

    Ok(())
}

// ---------------------------------------------------------------------------
// build logs get
// ---------------------------------------------------------------------------

async fn cmd_get(
    args: GetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let output_path = args
        .output
        .unwrap_or_else(|| format!("build-{}.log", args.id));

    let mut resp = client
        .get_stream(&format!("builds/{}/logs", args.id))
        .await?;

    let mut file = tokio::fs::File::create(&output_path)
        .await
        .map_err(|e| Error::Other(format!("cannot create '{output_path}': {e}")))?;

    let mut total_bytes: u64 = 0;

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| Error::Connection(format!("download error: {e}")))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| Error::Other(format!("write error: {e}")))?;
        total_bytes += chunk.len() as u64;
    }

    println!(
        "log written to {output_path} ({})",
        format_size(total_bytes)
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a byte count as a human-readable string.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
