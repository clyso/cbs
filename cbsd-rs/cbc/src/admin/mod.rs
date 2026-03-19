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

//! Admin commands: role management, user management, and build queue status.

pub mod queue;
pub mod roles;
pub mod users;

use clap::{Args, Subcommand};

use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct AdminArgs {
    #[command(subcommand)]
    command: AdminCommands,
}

#[derive(Subcommand)]
enum AdminCommands {
    /// Role management
    Roles(roles::RolesArgs),
    /// User management and role assignments
    Users(users::UsersArgs),
    /// Build queue status
    Queue,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: AdminArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    match args.command {
        AdminCommands::Roles(a) => roles::run(a, config_path, debug).await,
        AdminCommands::Users(a) => users::run(a, config_path, debug).await,
        AdminCommands::Queue => queue::run(config_path, debug).await,
    }
}
