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

//! Role CRUD commands: list, create, get, update, delete.

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct RolesArgs {
    #[command(subcommand)]
    command: RolesCommands,
}

#[derive(Subcommand)]
enum RolesCommands {
    /// List all roles
    List,
    /// Create a new role
    Create(CreateArgs),
    /// Show role details including capabilities
    Get(GetArgs),
    /// Update a role's capabilities (replaces entire set)
    Update(UpdateArgs),
    /// Delete a role
    Delete(DeleteArgs),
}

#[derive(Args)]
struct CreateArgs {
    /// Role name
    name: String,
    /// Capability to grant (repeatable)
    #[arg(long = "cap")]
    caps: Vec<String>,
    /// Scope in TYPE=PATTERN format (repeatable, e.g. channel=ces-devel/*)
    #[arg(long)]
    scope: Vec<String>,
    /// Role description
    #[arg(long)]
    description: Option<String>,
}

#[derive(Args)]
struct GetArgs {
    /// Role name
    name: String,
}

#[derive(Args)]
struct UpdateArgs {
    /// Role name
    name: String,
    /// Capability to set (repeatable, replaces entire set)
    #[arg(long = "cap")]
    caps: Vec<String>,
    /// Scope in TYPE=PATTERN format (repeatable, replaces entire set)
    #[arg(long)]
    scope: Vec<String>,
}

#[derive(Args)]
struct DeleteArgs {
    /// Role name
    name: String,
    /// Delete even if role has active assignments
    #[arg(long)]
    force: bool,
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RoleListItem {
    name: String,
    description: String,
    builtin: bool,
    #[allow(dead_code)]
    created_at: i64,
}

#[derive(Deserialize)]
struct RoleDetail {
    name: String,
    description: String,
    builtin: bool,
    caps: Vec<String>,
    #[serde(default)]
    scopes: Vec<ScopeItem>,
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct ScopeItem {
    #[serde(rename = "type")]
    pub(crate) scope_type: String,
    pub(crate) pattern: String,
}

#[derive(Serialize)]
struct CreateRoleBody {
    name: String,
    caps: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scopes: Vec<ScopeItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct UpdateRoleBody {
    name: String,
    caps: Vec<String>,
    scopes: Vec<ScopeItem>,
}

#[derive(Deserialize)]
struct SimpleResponse {
    #[allow(dead_code)]
    #[serde(default)]
    detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Scope parsing
// ---------------------------------------------------------------------------

/// Parse a scope string in the format `TYPE=PATTERN`.
pub(crate) fn parse_scope(s: &str) -> Result<ScopeItem, Error> {
    let (scope_type, pattern) = s.split_once('=').ok_or_else(|| {
        Error::Other(format!(
            "invalid scope '{s}': expected TYPE=PATTERN (e.g. channel=ces-devel/*)"
        ))
    })?;

    if scope_type.is_empty() || pattern.is_empty() {
        return Err(Error::Other(format!(
            "invalid scope '{s}': both type and pattern must be non-empty"
        )));
    }

    Ok(ScopeItem {
        scope_type: scope_type.to_string(),
        pattern: pattern.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: RolesArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    match args.command {
        RolesCommands::List => cmd_list(config_path, debug, no_tls_verify).await,
        RolesCommands::Create(a) => cmd_create(a, config_path, debug, no_tls_verify).await,
        RolesCommands::Get(a) => cmd_get(a, config_path, debug, no_tls_verify).await,
        RolesCommands::Update(a) => cmd_update(a, config_path, debug, no_tls_verify).await,
        RolesCommands::Delete(a) => cmd_delete(a, config_path, debug, no_tls_verify).await,
    }
}

// ---------------------------------------------------------------------------
// admin roles list
// ---------------------------------------------------------------------------

async fn cmd_list(
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let roles: Vec<RoleListItem> = client.get("permissions/roles").await?;

    if roles.is_empty() {
        println!("no roles found");
        return Ok(());
    }

    println!("  {:<18} {:<9} DESCRIPTION", "NAME", "BUILTIN",);

    for role in &roles {
        let builtin = if role.builtin { "yes" } else { "no" };
        println!("  {:<18} {:<9} {}", role.name, builtin, role.description);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// admin roles create
// ---------------------------------------------------------------------------

async fn cmd_create(
    args: CreateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let scopes: Vec<ScopeItem> = args
        .scope
        .iter()
        .map(|s| parse_scope(s))
        .collect::<Result<Vec<_>, _>>()?;

    let body = CreateRoleBody {
        name: args.name.clone(),
        caps: args.caps,
        scopes,
        description: args.description,
    };

    let _resp: SimpleResponse = client.post("permissions/roles", &body).await?;

    println!("role '{}' created", args.name);
    Ok(())
}

// ---------------------------------------------------------------------------
// admin roles get
// ---------------------------------------------------------------------------

async fn cmd_get(
    args: GetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let role: RoleDetail = client
        .get(&format!("permissions/roles/{}", args.name))
        .await?;

    let builtin = if role.builtin { "yes" } else { "no" };

    println!("      name: {}", role.name);
    println!("   builtin: {builtin}");
    println!("      desc: {}", role.description);

    if role.caps.is_empty() {
        println!("      caps: (none)");
    } else {
        for (i, cap) in role.caps.iter().enumerate() {
            if i == 0 {
                println!("      caps: {cap}");
            } else {
                println!("            {cap}");
            }
        }
    }

    if role.scopes.is_empty() {
        println!("    scopes: (none)");
    } else {
        for (i, scope) in role.scopes.iter().enumerate() {
            let entry = format!("{} = {}", scope.scope_type, scope.pattern);
            if i == 0 {
                println!("    scopes: {entry}");
            } else {
                println!("            {entry}");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// admin roles update
// ---------------------------------------------------------------------------

async fn cmd_update(
    args: UpdateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let scopes: Vec<ScopeItem> = args
        .scope
        .iter()
        .map(|s| parse_scope(s))
        .collect::<Result<Vec<_>, _>>()?;

    let n_caps = args.caps.len();
    let n_scopes = scopes.len();
    let body = UpdateRoleBody {
        name: args.name.clone(),
        caps: args.caps,
        scopes,
    };

    match client
        .put_json::<SimpleResponse>(&format!("permissions/roles/{}", args.name), &body)
        .await
    {
        Ok(_) => {
            println!(
                "role '{}' updated ({n_caps} capabilities, {n_scopes} scopes)",
                args.name
            );
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
// admin roles delete
// ---------------------------------------------------------------------------

async fn cmd_delete(
    args: DeleteArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let path = if args.force {
        format!("permissions/roles/{}?force=true", args.name)
    } else {
        format!("permissions/roles/{}", args.name)
    };

    match client.delete::<SimpleResponse>(&path).await {
        Ok(_) => {
            println!("role '{}' deleted", args.name);
            Ok(())
        }
        Err(Error::Api {
            status: 409,
            message,
        }) => {
            if !args.force {
                eprintln!("role has active assignments -- use --force");
            } else {
                eprintln!("error: {message}");
            }
            Err(Error::Api {
                status: 409,
                message,
            })
        }
        Err(e) => Err(e),
    }
}
