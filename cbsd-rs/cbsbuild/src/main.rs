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
//! `cbscore`'s `cbsbuild` entry point).
//!
//! This is the C0 scaffold: the clap root and the global `--config` /
//! `--debug` placeholders. The `build`, `runner build`, and `versions`
//! subcommands land by capability in later commits (design 010 owns the full
//! command tree). The binary is built static-musl and mounted into the builder
//! container by the runner; designs 001 and 012 govern that portability gate.

use std::path::PathBuf;

use clap::Parser;

/// Extended version: cargo version + git SHA from the production build.
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("CBS_BUILD_META"),);

/// CES Build System build CLI.
#[derive(Parser)]
#[command(name = "cbsbuild", version = VERSION, about = "CES Build System build CLI")]
struct Cli {
    /// Path to the cbsbuild config file. Read by the commands that load it
    /// (`build`, `versions list`); `versions create` ignores it (design 010).
    #[arg(short = 'c', long, default_value = "cbs-build.config.yaml")]
    config: PathBuf,

    /// Enable debug logging. The commands wire this into the tracing subscriber
    /// (and honour the `CBS_DEBUG` env var) in M1; for now it reports versions.
    #[arg(short = 'd', long)]
    debug: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // C0 scaffold: the build/versions subcommands are not wired yet (M1+).
    // `--debug` exercises the global flags and the linked crate stack by
    // reporting the CLI and library versions.
    if cli.debug {
        eprintln!("cbsbuild {VERSION} (cbscore {})", cbscore::VERSION);
        eprintln!("config: {}", cli.config.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // clap's own invariant checker: catches conflicting flags, duplicate
        // short/long names, and bad defaults at test time.
        Cli::command().debug_assert();
    }
}
