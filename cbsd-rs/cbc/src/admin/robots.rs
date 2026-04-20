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

//! Robot account management commands.

use chrono::NaiveDate;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct RobotsArgs {
    #[command(subcommand)]
    command: RobotsCommands,
}

#[derive(Subcommand)]
enum RobotsCommands {
    /// Create a new robot account (or revive a tombstoned one)
    Create(CreateArgs),
    /// List all robot accounts
    List,
    /// Show robot account details
    Get(NameArgs),
    /// Set or clear a robot's description
    SetDescription(SetDescriptionArgs),
    /// Enable (re-activate) a robot account
    Enable(NameArgs),
    /// Disable (deactivate) a robot account
    Disable(NameArgs),
    /// Manage role assignments for a robot
    Roles(RolesArgs),
    /// Manage the robot's default build channel
    DefaultChannel(DefaultChannelArgs),
    /// Manage a robot's bearer token
    Token(TokenArgs),
    /// Tombstone (permanently delete) a robot account
    Delete(DeleteArgs),
}

#[derive(Args)]
struct NameArgs {
    /// Robot name
    name: String,
}

#[derive(Args)]
struct CreateArgs {
    /// Robot name (alphanumeric, hyphens, underscores; max 64 chars)
    name: String,
    /// Token expiry date in YYYY-MM-DD format.
    ///
    /// Dates are interpreted as UTC. A token issued with `--expires
    /// 2026-12-31` is valid through the end of 2026-12-31 UTC — the
    /// server stores it as the epoch of 2027-01-01 00:00:00 UTC.
    #[arg(
        long,
        conflicts_with = "no_expires",
        required_unless_present = "no_expires"
    )]
    expires: Option<String>,
    /// Issue a token that never expires. Mutually exclusive with
    /// `--expires`; one of the two is required so a forever-token is
    /// always an explicit caller opt-in.
    #[arg(long = "no-expires", conflicts_with = "expires")]
    no_expires: bool,
    /// Robot description
    #[arg(long)]
    description: Option<String>,
    /// Role to assign (repeatable)
    #[arg(long)]
    role: Vec<String>,
}

#[derive(Args)]
struct SetDescriptionArgs {
    /// Robot name
    name: String,
    /// New description (omit to clear)
    #[arg(long)]
    description: Option<String>,
}

#[derive(Args)]
struct DeleteArgs {
    /// Robot name
    name: String,
    /// Confirm this irreversible operation
    #[arg(long = "yes-i-really-mean-it")]
    yes_i_really_mean_it: bool,
}

#[derive(Args)]
struct RolesArgs {
    #[command(subcommand)]
    command: RolesCommands,
}

#[derive(Subcommand)]
enum RolesCommands {
    /// Replace all roles for a robot
    Set(RolesSetArgs),
    /// Add a role to a robot
    Add(RolesAddArgs),
    /// Remove a role from a robot
    Remove(RolesRemoveArgs),
}

#[derive(Args)]
struct RolesSetArgs {
    /// Robot name
    name: String,
    /// Role name (repeatable)
    #[arg(long)]
    role: Vec<String>,
}

#[derive(Args)]
struct RolesAddArgs {
    /// Robot name
    name: String,
    /// Role name to add
    #[arg(long)]
    role: String,
}

#[derive(Args)]
struct RolesRemoveArgs {
    /// Robot name
    name: String,
    /// Role name to remove
    #[arg(long)]
    role: String,
}

#[derive(Args)]
struct DefaultChannelArgs {
    #[command(subcommand)]
    command: DefaultChannelCommands,
}

#[derive(Subcommand)]
enum DefaultChannelCommands {
    /// Set the robot's default build channel
    Set(DefaultChannelSetArgs),
    /// Clear the robot's default build channel
    Clear(NameArgs),
}

#[derive(Args)]
struct DefaultChannelSetArgs {
    /// Robot name
    name: String,
    /// Channel ID
    #[arg(long)]
    channel: i64,
}

#[derive(Args)]
struct TokenArgs {
    #[command(subcommand)]
    command: TokenCommands,
}

#[derive(Subcommand)]
enum TokenCommands {
    /// Issue a new token (requires --renew if one already exists)
    New(TokenNewArgs),
    /// Revoke all active tokens for a robot
    Revoke(TokenRevokeArgs),
}

#[derive(Args)]
struct TokenNewArgs {
    /// Robot name
    name: String,
    /// Token expiry date in YYYY-MM-DD format (UTC; see `robots create`
    /// for the UTC day-after semantics).
    #[arg(
        long,
        conflicts_with = "no_expires",
        required_unless_present = "no_expires"
    )]
    expires: Option<String>,
    /// Issue a token that never expires. Mutually exclusive with
    /// `--expires`; one of the two is required.
    #[arg(long = "no-expires", conflicts_with = "expires")]
    no_expires: bool,
    /// Replace an existing non-revoked token
    #[arg(long)]
    renew: bool,
}

#[derive(Args)]
struct TokenRevokeArgs {
    /// Robot name
    name: String,
    /// Confirm this irreversible operation
    #[arg(long = "yes-i-really-mean-it")]
    yes_i_really_mean_it: bool,
}

// ---------------------------------------------------------------------------
// API request / response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CreateRobotBody {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Design v4 wire format: `"YYYY-MM-DD"` string or JSON `null`.
    /// The field is always serialized (`skip_serializing_if` is
    /// deliberately absent) so an explicit `null` is sent for
    /// never-expiring tokens rather than field omission.
    expires: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    roles: Vec<String>,
}

#[derive(Deserialize)]
struct CreateRobotResponse {
    name: String,
    #[allow(dead_code)]
    display_name: String,
    email: String,
    #[allow(dead_code)]
    active: bool,
    #[allow(dead_code)]
    description: Option<String>,
    token: String,
    token_prefix: String,
    token_expires_at: Option<i64>,
    #[allow(dead_code)]
    created_at: i64,
    roles: Vec<String>,
    revived: bool,
}

#[derive(Deserialize)]
struct RobotListItem {
    name: String,
    display_name: String,
    email: String,
    #[allow(dead_code)]
    description: Option<String>,
    active: bool,
    #[allow(dead_code)]
    created_at: i64,
    token_state: String,
    #[serde(default)]
    token_expires_at: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    last_used_at: Option<i64>,
}

#[derive(Deserialize)]
struct RobotDetailResponse {
    name: String,
    display_name: String,
    email: String,
    description: Option<String>,
    active: bool,
    #[allow(dead_code)]
    created_at: i64,
    token_status: TokenStatusBody,
    roles: Vec<String>,
    effective_caps: Vec<String>,
}

#[derive(Deserialize)]
struct TokenStatusBody {
    state: String,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    first_used_at: Option<i64>,
    #[serde(default)]
    last_used_at: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    token_created_at: Option<i64>,
}

#[derive(Serialize)]
struct SetDescriptionBody {
    description: Option<String>,
}

#[derive(Serialize)]
struct RotateTokenBody {
    /// Design v4 wire format — see [`CreateRobotBody::expires`].
    expires: Option<String>,
    renew: bool,
}

#[derive(Deserialize)]
struct RotateTokenResponse {
    token: String,
    token_prefix: String,
    expires_at: Option<i64>,
}

#[derive(Serialize)]
struct ReplaceRolesBody {
    roles: Vec<String>,
}

#[derive(Serialize)]
struct AddRoleBody {
    role: String,
}

#[derive(Serialize)]
struct SetDefaultChannelBody {
    channel_id: Option<i64>,
}

#[derive(Deserialize)]
struct SimpleResponse {
    #[allow(dead_code)]
    #[serde(default)]
    detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn name_to_synthetic_email(name: &str) -> String {
    format!("robot+{name}@robots")
}

/// Build the wire value for the `expires` field from a pair of flags.
/// Exactly one of `expires` / `no_expires` is required at the clap layer;
/// this function encodes the pair into the JSON value the server expects
/// (`Some(date)` → string, `None` → JSON `null`). Validates that the
/// date string parses as `YYYY-MM-DD` so a typo surfaces before the
/// request is sent.
fn build_expires_wire(expires: Option<String>, no_expires: bool) -> Result<Option<String>, Error> {
    if no_expires {
        return Ok(None);
    }
    let date = expires.ok_or_else(|| {
        Error::Other("internal: one of --expires or --no-expires must be set".into())
    })?;
    NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|_| {
        Error::Other(format!(
            "invalid date '{date}': expected YYYY-MM-DD (UTC; valid through end of that UTC day)"
        ))
    })?;
    Ok(Some(date))
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: RobotsArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    match args.command {
        RobotsCommands::Create(a) => cmd_create(a, config_path, debug, no_tls_verify).await,
        RobotsCommands::List => cmd_list(config_path, debug, no_tls_verify).await,
        RobotsCommands::Get(a) => cmd_get(a, config_path, debug, no_tls_verify).await,
        RobotsCommands::SetDescription(a) => {
            cmd_set_description(a, config_path, debug, no_tls_verify).await
        }
        RobotsCommands::Enable(a) => cmd_enable(a, config_path, debug, no_tls_verify).await,
        RobotsCommands::Disable(a) => cmd_disable(a, config_path, debug, no_tls_verify).await,
        RobotsCommands::Roles(a) => match a.command {
            RolesCommands::Set(sa) => cmd_roles_set(sa, config_path, debug, no_tls_verify).await,
            RolesCommands::Add(sa) => cmd_roles_add(sa, config_path, debug, no_tls_verify).await,
            RolesCommands::Remove(sa) => {
                cmd_roles_remove(sa, config_path, debug, no_tls_verify).await
            }
        },
        RobotsCommands::DefaultChannel(a) => match a.command {
            DefaultChannelCommands::Set(sa) => {
                cmd_default_channel_set(sa, config_path, debug, no_tls_verify).await
            }
            DefaultChannelCommands::Clear(sa) => {
                cmd_default_channel_clear(sa, config_path, debug, no_tls_verify).await
            }
        },
        RobotsCommands::Token(a) => match a.command {
            TokenCommands::New(sa) => cmd_token_new(sa, config_path, debug, no_tls_verify).await,
            TokenCommands::Revoke(sa) => {
                cmd_token_revoke(sa, config_path, debug, no_tls_verify).await
            }
        },
        RobotsCommands::Delete(a) => cmd_delete(a, config_path, debug, no_tls_verify).await,
    }
}

// ---------------------------------------------------------------------------
// admin robots create
// ---------------------------------------------------------------------------

async fn cmd_create(
    args: CreateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let expires = build_expires_wire(args.expires, args.no_expires)?;

    let body = CreateRobotBody {
        name: args.name.clone(),
        description: args.description,
        expires,
        roles: args.role,
    };

    match client
        .post::<_, CreateRobotResponse>("admin/robots", &body)
        .await
    {
        Ok(resp) => {
            if resp.revived {
                eprintln!("note: tombstoned robot '{}' has been revived", resp.name);
            }
            println!("robot '{}' created", resp.name);
            println!("  email: {}", resp.email);
            if !resp.roles.is_empty() {
                println!("  roles: {}", resp.roles.join(", "));
            }
            match resp.token_expires_at {
                Some(exp) => println!("  expires: {exp} (unix timestamp)"),
                None => println!("  expires: never"),
            }
            println!();
            println!("  token prefix: {}", resp.token_prefix);
            println!("  token: {}", resp.token);
            println!();
            println!("  *** This token will not be shown again. Store it securely. ***");
            Ok(())
        }
        Err(Error::Api {
            status: 409,
            message,
        }) => {
            eprintln!("error: {message}");
            Err(Error::Api {
                status: 409,
                message,
            })
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// admin robots list
// ---------------------------------------------------------------------------

async fn cmd_list(
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let robots: Vec<RobotListItem> = client.get("admin/robots").await?;

    if robots.is_empty() {
        println!("no robots found");
        return Ok(());
    }

    println!(
        "  {:<24} {:<8} {:<10} {:<14} EMAIL",
        "NAME", "ACTIVE", "TOKEN", "EXPIRES",
    );

    for robot in &robots {
        let active = if robot.active { "yes" } else { "no" };
        let expires = match robot.token_expires_at {
            Some(exp) => exp.to_string(),
            None => "never".to_string(),
        };
        println!(
            "  {:<24} {:<8} {:<10} {:<14} {}  ({})",
            robot.name, active, robot.token_state, expires, robot.email, robot.display_name,
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots get
// ---------------------------------------------------------------------------

async fn cmd_get(
    args: NameArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    match client
        .get::<RobotDetailResponse>(&format!("admin/robots/{}", args.name))
        .await
    {
        Ok(robot) => {
            let active = if robot.active { "yes" } else { "no" };
            println!("          name: {}", robot.name);
            println!("  display name: {}", robot.display_name);
            println!("         email: {}", robot.email);
            println!("        active: {active}");
            if let Some(desc) = &robot.description {
                println!("   description: {desc}");
            }

            println!();
            println!("  token status:");
            println!("     state: {}", robot.token_status.state);
            if let Some(p) = &robot.token_status.prefix {
                println!("    prefix: {p}");
            }
            match robot.token_status.expires_at {
                Some(exp) => println!("    expires: {exp} (unix timestamp)"),
                None if robot.token_status.state != "none" => println!("    expires: never"),
                None => {}
            }
            if let Some(v) = robot.token_status.first_used_at {
                println!("    first used: {v} (unix timestamp)");
            }
            if let Some(v) = robot.token_status.last_used_at {
                println!("    last used:  {v} (unix timestamp)");
            }

            if !robot.roles.is_empty() {
                println!();
                println!("  roles: {}", robot.roles.join(", "));
            }
            if !robot.effective_caps.is_empty() {
                println!();
                println!("  effective caps:");
                for cap in &robot.effective_caps {
                    println!("    - {cap}");
                }
            }
            Ok(())
        }
        Err(Error::Api { status: 404, .. }) => {
            eprintln!("robot '{}' not found", args.name);
            Err(Error::Other(format!("robot '{}' not found", args.name)))
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// admin robots set-description
// ---------------------------------------------------------------------------

async fn cmd_set_description(
    args: SetDescriptionArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = SetDescriptionBody {
        description: args.description,
    };

    let _: SimpleResponse = client
        .put_json(&format!("admin/robots/{}/description", args.name), &body)
        .await?;

    println!("description updated for robot '{}'", args.name);
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots enable
// ---------------------------------------------------------------------------

async fn cmd_enable(
    args: NameArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);

    match client
        .put_empty::<SimpleResponse>(&format!("admin/entity/{email}/activate"))
        .await
    {
        Ok(_) => {
            println!("robot '{}' enabled", args.name);
            Ok(())
        }
        Err(Error::Api {
            status: 409,
            message,
        }) => {
            eprintln!("error: {message}");
            Err(Error::Api {
                status: 409,
                message,
            })
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// admin robots disable
// ---------------------------------------------------------------------------

async fn cmd_disable(
    args: NameArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);

    match client
        .put_empty::<serde_json::Value>(&format!("admin/entity/{email}/deactivate"))
        .await
    {
        Ok(_) => {
            println!("robot '{}' disabled", args.name);
            Ok(())
        }
        Err(Error::Api {
            status: 409,
            message,
        }) => {
            eprintln!("error: {message}");
            Err(Error::Api {
                status: 409,
                message,
            })
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// admin robots roles set
// ---------------------------------------------------------------------------

async fn cmd_roles_set(
    args: RolesSetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);
    let body = ReplaceRolesBody {
        roles: args.role.clone(),
    };

    let _: serde_json::Value = client
        .put_json(&format!("admin/entity/{email}/roles"), &body)
        .await?;

    println!("robot '{}' roles set: {}", args.name, args.role.join(", "));
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots roles add
// ---------------------------------------------------------------------------

async fn cmd_roles_add(
    args: RolesAddArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);
    let body = AddRoleBody {
        role: args.role.clone(),
    };

    let _: serde_json::Value = client
        .post(&format!("admin/entity/{email}/roles"), &body)
        .await?;

    println!("role '{}' added to robot '{}'", args.role, args.name);
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots roles remove
// ---------------------------------------------------------------------------

async fn cmd_roles_remove(
    args: RolesRemoveArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);
    let _: SimpleResponse = client
        .delete(&format!("admin/entity/{email}/roles/{}", args.role))
        .await?;

    println!("role '{}' removed from robot '{}'", args.role, args.name);
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots default-channel set
// ---------------------------------------------------------------------------

async fn cmd_default_channel_set(
    args: DefaultChannelSetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);
    let body = SetDefaultChannelBody {
        channel_id: Some(args.channel),
    };

    let resp: serde_json::Value = client
        .put_json(&format!("admin/entity/{email}/default-channel"), &body)
        .await?;

    if let Some(detail) = resp.get("detail").and_then(|v| v.as_str()) {
        println!("{detail}");
    } else {
        println!("default channel set for robot '{}'", args.name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots default-channel clear
// ---------------------------------------------------------------------------

async fn cmd_default_channel_clear(
    args: NameArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let email = name_to_synthetic_email(&args.name);
    let body = SetDefaultChannelBody { channel_id: None };

    let resp: serde_json::Value = client
        .put_json(&format!("admin/entity/{email}/default-channel"), &body)
        .await?;

    if let Some(detail) = resp.get("detail").and_then(|v| v.as_str()) {
        println!("{detail}");
    } else {
        println!("default channel cleared for robot '{}'", args.name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots token new
// ---------------------------------------------------------------------------

async fn cmd_token_new(
    args: TokenNewArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let expires = build_expires_wire(args.expires, args.no_expires)?;

    let body = RotateTokenBody {
        expires,
        renew: args.renew,
    };

    match client
        .post::<_, RotateTokenResponse>(&format!("admin/robots/{}/token", args.name), &body)
        .await
    {
        Ok(resp) => {
            match resp.expires_at {
                Some(exp) => println!("  expires: {exp} (unix timestamp)"),
                None => println!("  expires: never"),
            }
            println!("  token prefix: {}", resp.token_prefix);
            println!("  token: {}", resp.token);
            println!();
            println!("  *** This token will not be shown again. Store it securely. ***");
            Ok(())
        }
        Err(Error::Api {
            status: 409,
            message,
        }) => {
            eprintln!("error: {message}");
            eprintln!("hint: use --renew to replace an existing active token");
            Err(Error::Api {
                status: 409,
                message,
            })
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// admin robots token revoke
// ---------------------------------------------------------------------------

async fn cmd_token_revoke(
    args: TokenRevokeArgs,
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

    let resp: serde_json::Value = client
        .delete(&format!("admin/robots/{}/token", args.name))
        .await?;

    if let Some(detail) = resp.get("detail").and_then(|v| v.as_str()) {
        println!("{detail}");
    } else {
        println!("token(s) revoked for robot '{}'", args.name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// admin robots delete
// ---------------------------------------------------------------------------

async fn cmd_delete(
    args: DeleteArgs,
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

    match client
        .delete::<serde_json::Value>(&format!("admin/robots/{}", args.name))
        .await
    {
        Ok(resp) => {
            if let Some(detail) = resp.get("detail").and_then(|v| v.as_str()) {
                println!("{detail}");
            } else {
                println!("robot '{}' tombstoned", args.name);
            }
            Ok(())
        }
        Err(Error::Api { status: 404, .. }) => {
            eprintln!("robot '{}' not found", args.name);
            Err(Error::Other(format!("robot '{}' not found", args.name)))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_expires_wire_no_expires_yields_none() {
        let v = build_expires_wire(None, true).unwrap();
        assert!(v.is_none(), "--no-expires maps to JSON null");
    }

    #[test]
    fn build_expires_wire_iso_date_passes_through() {
        let v = build_expires_wire(Some("2026-12-31".to_string()), false).unwrap();
        assert_eq!(v.as_deref(), Some("2026-12-31"));
    }

    #[test]
    fn build_expires_wire_rejects_malformed_date() {
        let err =
            build_expires_wire(Some("not-a-date".to_string()), false).expect_err("expected error");
        let msg = format!("{err}");
        assert!(
            msg.contains("not-a-date"),
            "error should mention the bad input: {msg}"
        );
    }

    #[test]
    fn create_body_serialises_none_as_explicit_null() {
        // Design v4 requires the wire to always carry the `expires` field.
        let body = CreateRobotBody {
            name: "ci".to_string(),
            description: None,
            expires: None,
            roles: Vec::new(),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json.get("expires"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn create_body_serialises_date_as_string() {
        let body = CreateRobotBody {
            name: "ci".to_string(),
            description: None,
            expires: Some("2026-12-31".to_string()),
            roles: Vec::new(),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(
            json.get("expires"),
            Some(&serde_json::Value::String("2026-12-31".to_string())),
        );
    }

    #[test]
    fn rotate_body_serialises_none_as_explicit_null() {
        let body = RotateTokenBody {
            expires: None,
            renew: true,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json.get("expires"), Some(&serde_json::Value::Null));
    }
}
