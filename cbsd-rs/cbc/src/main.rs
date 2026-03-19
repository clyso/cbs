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

pub mod builds;
mod client;
mod config;
mod error;
pub mod logs;
pub mod periodic;

use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Deserialize;

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

#[derive(Parser)]
#[command(name = "cbc", version, about = "CBS build service client")]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Enable debug output (print HTTP requests/responses)
    #[arg(short, long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with a CBS server
    Login {
        /// Server URL (e.g. https://cbs.example.com)
        url: String,
    },
    /// Show current user identity and roles
    Whoami,
    /// Build submission, listing, and management
    Build(Box<builds::BuildArgs>),
    /// Manage periodic (cron-scheduled) build tasks
    Periodic(periodic::PeriodicArgs),
}

#[derive(Deserialize)]
struct WhoamiResponse {
    email: String,
    name: String,
    roles: Vec<String>,
    effective_caps: Vec<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Error> {
    match cli.command {
        Commands::Login { url } => cmd_login(&url, cli.debug).await,
        Commands::Whoami => cmd_whoami(cli.config.as_deref(), cli.debug).await,
        Commands::Build(args) => builds::run(*args, cli.config.as_deref(), cli.debug).await,
        Commands::Periodic(args) => periodic::run(args, cli.config.as_deref(), cli.debug).await,
    }
}

async fn cmd_login(url: &str, debug: bool) -> Result<(), Error> {
    // Verify server is reachable.
    let client = CbcClient::unauthenticated(url, debug)?;
    client
        .get::<serde_json::Value>("health")
        .await
        .map_err(|_| Error::Connection(format!("cannot reach server at {url}")))?;

    // Direct the user to the login page.
    let login_url = format!("{}/api/auth/login?client=cli", url.trim_end_matches('/'));
    let _ = open::that(&login_url);
    eprintln!("Open this URL in your browser to log in:\n  {login_url}\n");

    // Prompt for the token.
    eprint!("Paste the token here: ");
    std::io::stderr()
        .flush()
        .map_err(|e| Error::Other(e.to_string()))?;

    let mut token = String::new();
    std::io::stdin()
        .read_line(&mut token)
        .map_err(|e| Error::Other(format!("cannot read token: {e}")))?;
    let token = token.trim().to_string();

    if token.is_empty() {
        return Err(Error::Config("no token provided".into()));
    }

    // Validate the token.
    let client = CbcClient::new(url, &token, debug)?;
    let whoami: serde_json::Value = client
        .get("auth/whoami")
        .await
        .map_err(|_| Error::Config("invalid token".into()))?;

    let email = whoami
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Persist.
    let config_path = Config::default_path()
        .ok_or_else(|| Error::Config("cannot determine config directory".into()))?;

    let cfg = Config {
        host: url.to_string(),
        token,
    };
    cfg.save(&config_path)?;

    eprintln!("logged in as {email}");
    Ok(())
}

async fn cmd_whoami(config_path: Option<&std::path::Path>, debug: bool) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    match client.get::<WhoamiResponse>("auth/whoami").await {
        Ok(w) => {
            println!("email: {}", w.email);
            println!("name:  {}", w.name);
            println!("roles: {}", w.roles.join(", "));
            println!("caps:  {}", w.effective_caps.join(", "));
            Ok(())
        }
        Err(Error::Api { status: 401, .. }) => {
            eprintln!(
                "session expired — run 'cbc login {}' to re-authenticate",
                config.host
            );
            Err(Error::Api {
                status: 401,
                message: "session expired".into(),
            })
        }
        Err(e) => Err(e),
    }
}
