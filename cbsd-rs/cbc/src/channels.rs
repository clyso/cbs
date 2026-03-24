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

//! Channel listing: `cbc channel list`.

use clap::{Args, Subcommand};
use serde::Deserialize;

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ChannelArgs {
    #[command(subcommand)]
    command: ChannelCommands,
}

#[derive(Subcommand)]
enum ChannelCommands {
    /// List channels and their types
    List,
}

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChannelResponse {
    id: i64,
    name: String,
    description: String,
    default_type_id: Option<i64>,
    types: Vec<TypeResponse>,
}

#[derive(Deserialize)]
struct TypeResponse {
    id: i64,
    type_name: String,
    project: String,
    prefix_template: String,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: ChannelArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    match args.command {
        ChannelCommands::List => cmd_list(config_path, debug).await,
    }
}

// ---------------------------------------------------------------------------
// channel list
// ---------------------------------------------------------------------------

async fn cmd_list(config_path: Option<&std::path::Path>, debug: bool) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let channels: Vec<ChannelResponse> = client.get("channels").await?;

    if channels.is_empty() {
        println!("no channels found");
        return Ok(());
    }

    for channel in &channels {
        println!(
            "  {} (id={}){}",
            channel.name,
            channel.id,
            if channel.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", channel.description)
            },
        );

        if channel.types.is_empty() {
            println!("    (no types configured)");
        } else {
            for t in &channel.types {
                let is_default = channel
                    .default_type_id
                    .is_some_and(|did| did == t.id);
                let marker = if is_default { " [default]" } else { "" };
                let prefix_display = if t.prefix_template.is_empty() {
                    String::new()
                } else {
                    format!(", prefix={}", t.prefix_template)
                };
                println!(
                    "    {:<8} project={}{}{marker}",
                    t.type_name, t.project, prefix_display,
                );
            }
        }
    }

    Ok(())
}
