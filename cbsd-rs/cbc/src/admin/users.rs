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

//! User management and user-role assignment commands.

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct UsersArgs {
    #[command(subcommand)]
    command: UsersCommands,
}

#[derive(Subcommand)]
enum UsersCommands {
    /// List all users with their roles
    List,
    /// Show user details including scopes and effective capabilities
    Get(GetArgs),
    /// Activate a user account
    Activate(ActivateArgs),
    /// Deactivate a user account (revokes tokens and API keys)
    Deactivate(DeactivateArgs),
    /// Manage user role assignments
    Roles(UserRolesArgs),
}

#[derive(Args)]
struct GetArgs {
    /// User email address
    email: String,
}

#[derive(Args)]
struct ActivateArgs {
    /// User email address
    email: String,
}

#[derive(Args)]
struct DeactivateArgs {
    /// User email address
    email: String,
}

#[derive(Args)]
struct UserRolesArgs {
    #[command(subcommand)]
    command: UserRolesCommands,
}

#[derive(Subcommand)]
enum UserRolesCommands {
    /// Replace all roles for a user
    Set(RolesSetArgs),
    /// Add a role to a user
    Add(RolesAddArgs),
    /// Remove a role from a user
    Remove(RolesRemoveArgs),
}

#[derive(Args)]
struct RolesSetArgs {
    /// User email address
    email: String,
    /// Role name (repeatable)
    #[arg(long)]
    role: Vec<String>,
}

#[derive(Args)]
struct RolesAddArgs {
    /// User email address
    email: String,
    /// Role name
    #[arg(long)]
    role: String,
}

#[derive(Args)]
struct RolesRemoveArgs {
    /// User email address
    email: String,
    /// Role name to remove
    #[arg(long)]
    role: String,
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UserWithRoles {
    email: String,
    name: String,
    active: bool,
    roles: Vec<UserRoleItem>,
}

#[derive(Deserialize)]
struct UserRoleItem {
    role: String,
    scopes: Vec<ScopeItem>,
}

use super::roles::ScopeItem;

#[derive(Serialize)]
struct ReplaceUserRolesBody {
    roles: Vec<String>,
}

#[derive(Serialize)]
struct AddUserRoleBody {
    role: String,
}

#[derive(Deserialize)]
struct DeactivateResponse {
    #[allow(dead_code)]
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    tokens_revoked: u64,
    #[serde(default)]
    api_keys_revoked: u64,
}

#[derive(Deserialize)]
struct SimpleResponse {
    #[allow(dead_code)]
    #[serde(default)]
    detail: Option<String>,
}

/// Role detail response (for fetching effective capabilities).
#[derive(Deserialize)]
struct RoleDetail {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    description: String,
    #[allow(dead_code)]
    builtin: bool,
    caps: Vec<String>,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: UsersArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    match args.command {
        UsersCommands::List => cmd_list(config_path, debug, no_tls_verify).await,
        UsersCommands::Get(a) => cmd_get(a, config_path, debug, no_tls_verify).await,
        UsersCommands::Activate(a) => cmd_activate(a, config_path, debug, no_tls_verify).await,
        UsersCommands::Deactivate(a) => cmd_deactivate(a, config_path, debug, no_tls_verify).await,
        UsersCommands::Roles(a) => match a.command {
            UserRolesCommands::Set(sa) => {
                cmd_roles_set(sa, config_path, debug, no_tls_verify).await
            }
            UserRolesCommands::Add(sa) => {
                cmd_roles_add(sa, config_path, debug, no_tls_verify).await
            }
            UserRolesCommands::Remove(sa) => {
                cmd_roles_remove(sa, config_path, debug, no_tls_verify).await
            }
        },
    }
}

// ---------------------------------------------------------------------------
// admin users list
// ---------------------------------------------------------------------------

async fn cmd_list(
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let users: Vec<UserWithRoles> = client.get("permissions/users").await?;

    if users.is_empty() {
        println!("no users found");
        return Ok(());
    }

    println!("  {:<24} {:<12} {:<8} ROLES", "EMAIL", "NAME", "ACTIVE",);

    for user in &users {
        let active = if user.active { "yes" } else { "no" };
        let role_names: Vec<&str> = user.roles.iter().map(|r| r.role.as_str()).collect();
        println!(
            "  {:<24} {:<12} {:<8} {}",
            user.email,
            user.name,
            active,
            role_names.join(", "),
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// admin users get
// ---------------------------------------------------------------------------

async fn cmd_get(
    args: GetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let users: Vec<UserWithRoles> = client.get("permissions/users").await?;

    let user = users.iter().find(|u| u.email == args.email);
    let user = match user {
        Some(u) => u,
        None => {
            eprintln!("user '{}' not found", args.email);
            return Err(Error::Other(format!("user '{}' not found", args.email)));
        }
    };

    let active = if user.active { "yes" } else { "no" };

    println!("   email: {}", user.email);
    println!("    name: {}", user.name);
    println!("  active: {active}");

    // Roles with scopes
    if user.roles.is_empty() {
        println!("\n  roles: (none)");
    } else {
        println!("\n  roles:");
        for role in &user.roles {
            println!("    {}", role.role);
            if role.scopes.is_empty() {
                println!("      (no scopes)");
            } else {
                println!("      scopes:");
                for scope in &role.scopes {
                    println!("        {} = {}", scope.scope_type, scope.pattern);
                }
            }
        }
    }

    // Effective capabilities: fetch each role's caps and deduplicate.
    let mut all_caps = Vec::new();
    for role in &user.roles {
        match client
            .get::<RoleDetail>(&format!("permissions/roles/{}", role.role))
            .await
        {
            Ok(detail) => {
                for cap in detail.caps {
                    if !all_caps.contains(&cap) {
                        all_caps.push(cap);
                    }
                }
            }
            Err(_) => {
                // Role may have been deleted between requests; skip.
            }
        }
    }

    if all_caps.is_empty() {
        println!("\n  effective caps: (none)");
    } else {
        println!("\n  effective caps:");
        println!("    {}", all_caps.join(", "));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// admin users activate
// ---------------------------------------------------------------------------

async fn cmd_activate(
    args: ActivateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let _resp: SimpleResponse = client
        .put_empty(&format!("admin/users/{}/activate", args.email))
        .await?;

    println!("user '{}' activated", args.email);
    Ok(())
}

// ---------------------------------------------------------------------------
// admin users deactivate
// ---------------------------------------------------------------------------

async fn cmd_deactivate(
    args: DeactivateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    match client
        .put_empty::<DeactivateResponse>(&format!("admin/users/{}/deactivate", args.email))
        .await
    {
        Ok(resp) => {
            println!("user '{}' deactivated", args.email);
            println!("  tokens revoked: {}", resp.tokens_revoked);
            println!("  api keys revoked: {}", resp.api_keys_revoked);
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
// admin users roles set
// ---------------------------------------------------------------------------

async fn cmd_roles_set(
    args: RolesSetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    if args.role.is_empty() {
        return Err(Error::Other("at least one --role is required".to_string()));
    }

    let body = ReplaceUserRolesBody {
        roles: args.role.clone(),
    };

    let _resp: serde_json::Value = client
        .put_json(&format!("permissions/users/{}/roles", args.email), &body)
        .await?;

    println!("user '{}' roles set: {}", args.email, args.role.join(", "));
    Ok(())
}

// ---------------------------------------------------------------------------
// admin users roles add
// ---------------------------------------------------------------------------

async fn cmd_roles_add(
    args: RolesAddArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let body = AddUserRoleBody {
        role: args.role.clone(),
    };

    let _resp: serde_json::Value = client
        .post(&format!("permissions/users/{}/roles", args.email), &body)
        .await?;

    println!("role '{}' added to user '{}'", args.role, args.email);
    Ok(())
}

// ---------------------------------------------------------------------------
// admin users roles remove
// ---------------------------------------------------------------------------

async fn cmd_roles_remove(
    args: RolesRemoveArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let _resp: SimpleResponse = client
        .delete(&format!(
            "permissions/users/{}/roles/{}",
            args.email, args.role,
        ))
        .await?;

    println!("role '{}' removed from '{}'", args.role, args.email);
    Ok(())
}
