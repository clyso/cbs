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

//! Admin channel and type management: create, update, delete channels and
//! types, set default type, set user default channel.

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ChannelAdminArgs {
    #[command(subcommand)]
    command: ChannelAdminCommands,
}

#[derive(Subcommand)]
enum ChannelAdminCommands {
    /// Create a new channel
    Create(ChannelCreateArgs),
    /// Update a channel
    Update(ChannelUpdateArgs),
    /// Delete a channel (soft-delete)
    Delete(ChannelDeleteArgs),
    /// Manage channel types
    Type(TypeAdminArgs),
    /// Set the default type for a channel
    SetDefaultType(SetDefaultTypeArgs),
}

#[derive(Args)]
struct ChannelCreateArgs {
    /// Channel name
    name: String,
    /// Optional description
    #[arg(long)]
    description: Option<String>,
}

#[derive(Args)]
struct ChannelUpdateArgs {
    /// Channel ID
    id: i64,
    /// New name
    #[arg(long)]
    name: Option<String>,
    /// New description
    #[arg(long)]
    description: Option<String>,
}

#[derive(Args)]
struct ChannelDeleteArgs {
    /// Channel ID
    id: i64,
}

#[derive(Args)]
struct SetDefaultTypeArgs {
    /// Channel ID
    channel_id: i64,
    /// Type ID to set as default
    type_id: i64,
}

#[derive(Args)]
struct TypeAdminArgs {
    #[command(subcommand)]
    command: TypeAdminCommands,
}

#[derive(Subcommand)]
enum TypeAdminCommands {
    /// Add a type to a channel
    Add(TypeAddArgs),
    /// Update a type's project/prefix
    Update(TypeUpdateArgs),
    /// Delete a type (soft-delete)
    Delete(TypeDeleteArgs),
}

#[derive(Args)]
struct TypeAddArgs {
    /// Channel ID
    channel_id: i64,
    /// Type name: dev, release, test, ci
    type_name: String,
    /// Harbor project name
    #[arg(long)]
    project: String,
    /// Prefix template (e.g. "${username}")
    #[arg(long, default_value = "")]
    prefix: String,
}

#[derive(Args)]
struct TypeUpdateArgs {
    /// Type ID
    type_id: i64,
    /// The channel ID the type belongs to
    #[arg(long)]
    channel_id: i64,
    /// New project
    #[arg(long)]
    project: Option<String>,
    /// New prefix template
    #[arg(long)]
    prefix: Option<String>,
}

#[derive(Args)]
struct TypeDeleteArgs {
    /// Type ID
    type_id: i64,
    /// The channel ID the type belongs to
    #[arg(long)]
    channel_id: i64,
}

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CreateChannelBody {
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
}

#[derive(Serialize)]
struct UpdateChannelBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct AddTypeBody {
    type_name: String,
    project: String,
    prefix_template: String,
}

#[derive(Serialize)]
struct UpdateTypeBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefix_template: Option<String>,
}

#[derive(Serialize)]
struct SetDefaultTypeBody {
    type_id: i64,
}

#[derive(Serialize)]
struct SetDefaultChannelBody {
    channel_id: i64,
}

#[derive(Deserialize)]
struct ChannelResponse {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct TypeResponseItem {
    id: i64,
    type_name: String,
    project: String,
}

#[derive(Deserialize)]
struct DetailResponse {
    detail: String,
}

// ---------------------------------------------------------------------------
// User default channel (separate from channel admin)
// ---------------------------------------------------------------------------

/// Set a user's default channel. Called from `cbc admin user set-default-channel`.
pub async fn set_user_default_channel(
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
    email: &str,
    channel_id: i64,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = SetDefaultChannelBody { channel_id };
    let resp: DetailResponse = client
        .put_json(&format!("admin/entity/{email}/default-channel"), &body)
        .await?;

    println!("{}", resp.detail);
    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: ChannelAdminArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    match args.command {
        ChannelAdminCommands::Create(a) => cmd_create(a, config_path, debug, no_tls_verify).await,
        ChannelAdminCommands::Update(a) => cmd_update(a, config_path, debug, no_tls_verify).await,
        ChannelAdminCommands::Delete(a) => cmd_delete(a, config_path, debug, no_tls_verify).await,
        ChannelAdminCommands::Type(a) => run_type(a, config_path, debug, no_tls_verify).await,
        ChannelAdminCommands::SetDefaultType(a) => {
            cmd_set_default_type(a, config_path, debug, no_tls_verify).await
        }
    }
}

async fn run_type(
    args: TypeAdminArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    match args.command {
        TypeAdminCommands::Add(a) => cmd_type_add(a, config_path, debug, no_tls_verify).await,
        TypeAdminCommands::Update(a) => cmd_type_update(a, config_path, debug, no_tls_verify).await,
        TypeAdminCommands::Delete(a) => cmd_type_delete(a, config_path, debug, no_tls_verify).await,
    }
}

// ---------------------------------------------------------------------------
// channel create
// ---------------------------------------------------------------------------

async fn cmd_create(
    args: ChannelCreateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = CreateChannelBody {
        name: args.name.clone(),
        description: args.description.unwrap_or_default(),
    };

    let resp: ChannelResponse = client.post("channels", &body).await?;
    println!("channel '{}' created (id={})", resp.name, resp.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// channel update
// ---------------------------------------------------------------------------

async fn cmd_update(
    args: ChannelUpdateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    if args.name.is_none() && args.description.is_none() {
        return Err(Error::Other(
            "at least one of --name or --description must be provided".to_string(),
        ));
    }

    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = UpdateChannelBody {
        name: args.name,
        description: args.description,
    };

    let resp: ChannelResponse = client
        .put_json(&format!("channels/{}", args.id), &body)
        .await?;

    println!("channel '{}' updated (id={})", resp.name, resp.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// channel delete
// ---------------------------------------------------------------------------

async fn cmd_delete(
    args: ChannelDeleteArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let resp: DetailResponse = client.delete(&format!("channels/{}", args.id)).await?;

    println!("{}", resp.detail);
    Ok(())
}

// ---------------------------------------------------------------------------
// channel type add
// ---------------------------------------------------------------------------

async fn cmd_type_add(
    args: TypeAddArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = AddTypeBody {
        type_name: args.type_name.clone(),
        project: args.project,
        prefix_template: args.prefix,
    };

    let resp: TypeResponseItem = client
        .post(&format!("channels/{}/types", args.channel_id), &body)
        .await?;

    println!(
        "type '{}' added to channel {} (type_id={}, project={})",
        resp.type_name, args.channel_id, resp.id, resp.project,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// channel type update
// ---------------------------------------------------------------------------

async fn cmd_type_update(
    args: TypeUpdateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    if args.project.is_none() && args.prefix.is_none() {
        return Err(Error::Other(
            "at least one of --project or --prefix must be provided".to_string(),
        ));
    }

    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = UpdateTypeBody {
        project: args.project,
        prefix_template: args.prefix,
    };

    let resp: TypeResponseItem = client
        .put_json(
            &format!("channels/{}/types/{}", args.channel_id, args.type_id),
            &body,
        )
        .await?;

    println!(
        "type '{}' updated (type_id={}, project={})",
        resp.type_name, resp.id, resp.project,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// channel type delete
// ---------------------------------------------------------------------------

async fn cmd_type_delete(
    args: TypeDeleteArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let resp: DetailResponse = client
        .delete(&format!(
            "channels/{}/types/{}",
            args.channel_id, args.type_id,
        ))
        .await?;

    println!("{}", resp.detail);
    Ok(())
}

// ---------------------------------------------------------------------------
// channel set-default-type
// ---------------------------------------------------------------------------

async fn cmd_set_default_type(
    args: SetDefaultTypeArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = SetDefaultTypeBody {
        type_id: args.type_id,
    };

    let resp: DetailResponse = client
        .put_json(&format!("channels/{}/default-type", args.channel_id), &body)
        .await?;

    println!("{}", resp.detail);
    Ok(())
}
