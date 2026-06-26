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

//! `cbsbuild` — the CES Build System build CLI (the Rust port of Python
//! `cbscore`'s `cbsbuild` entry point). It is a thin clap tree over the
//! `cbscore` library; each subcommand maps to a subsystem (design 010). The
//! `build`/`runner build` and `versions list` commands land in later milestones.

mod bool_parser;
mod cli;
mod cmds;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command, RunnerCommand, VersionsCommand};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(bool_parser::debug_enabled(cli.debug));

    match cli.command {
        Command::Versions { command } => match command {
            VersionsCommand::Create(args) => cmds::versions::create(&args).await,
        },
        Command::Build(args) => cmds::build::build(&cli.config, cli.debug, &args).await,
        Command::Runner { command } => match command {
            RunnerCommand::Build(args) => cmds::build::runner_build(&cli.config, &args).await,
        },
    }
}

/// Configure the tracing subscriber from the resolved debug level: `--debug`
/// (or a truthy `CBS_DEBUG`) enables the subsystem `DEBUG` spans; otherwise only
/// warnings and errors are shown. Logs go to stderr so command output on stdout
/// stays clean.
fn init_tracing(debug: bool) {
    let level = if debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::WARN
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .init();
}
