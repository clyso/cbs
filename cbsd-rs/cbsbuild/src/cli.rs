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

//! The `cbsbuild` clap command tree (design 010). M1 lands the root globals and
//! `versions create`; `build`, `runner build`, and `versions list` land with
//! their milestones.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};

/// Extended version: cargo version + git SHA from the production build.
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("CBS_BUILD_META"),);

/// CES Build System build CLI.
#[derive(Parser)]
#[command(name = "cbsbuild", version = VERSION, about = "CES Build System build CLI")]
pub struct Cli {
    /// Path to the cbsbuild config file. Read by the commands that load it
    /// (`build`, `versions list`); `versions create` ignores it. Deliberately
    /// **not** global, so it precedes the subcommand and leaves `-c` free for
    /// `versions create --component` (design 010).
    #[arg(short = 'c', long, default_value = "cbs-build.config.yaml")]
    pub config: PathBuf,

    /// Enable debug logging. Also honoured via the `CBS_DEBUG` env var, where
    /// `CBS_DEBUG=0` is off (design 010, H3).
    #[arg(short = 'd', long)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Subcommand)]
pub enum Command {
    /// Manipulate version descriptors.
    Versions {
        #[command(subcommand)]
        command: VersionsCommand,
    },
    /// Build a version descriptor (host side: spins the builder container).
    Build(BuildArgs),
    /// In-container build entry. Hidden: operators use `build` (design 010).
    #[command(hide = true)]
    Runner {
        #[command(subcommand)]
        command: RunnerCommand,
    },
}

/// `runner` subcommands (the in-container half; design 009/010).
#[derive(Subcommand)]
pub enum RunnerCommand {
    /// Run the in-container build for a mounted descriptor.
    Build(RunnerBuildArgs),
}

/// Coerce `--tls-verify`'s value with the Click-equivalent `BOOL` parser, so
/// `--tls-verify=yes`/`on`/`0` are accepted (not just clap's `true`/`false`).
fn parse_tls_bool(value: &str) -> Result<bool, String> {
    crate::bool_parser::parse_bool(value).ok_or_else(|| format!("invalid boolean value '{value}'"))
}

/// Flags for `build <DESCRIPTOR>` (design 010 §build).
#[derive(Args)]
pub struct BuildArgs {
    /// The version descriptor to build.
    pub descriptor: Utf8PathBuf,

    /// Build timeout in seconds (podman's `--timeout` and the await deadline).
    #[arg(long, default_value_t = 14400.0, value_name = "SECONDS")]
    pub timeout: f64,

    /// Override `config.signing.gpg` with this GPG signing id.
    #[arg(long, value_name = "ID")]
    pub sign_with_gpg_id: Option<String>,

    /// Override `config.signing.transit` with this Vault transit id.
    #[arg(long, value_name = "ID")]
    pub sign_with_transit: Option<String>,

    /// Write the build's container output to this file (must not exist).
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<Utf8PathBuf>,

    /// Skip running the per-component build scripts.
    #[arg(long)]
    pub skip_build: bool,

    /// Rebuild even when a release/image already exists.
    #[arg(long)]
    pub force: bool,

    /// Verify registry TLS. Value-taking: `--tls-verify=false` (design 010).
    #[arg(
        long,
        default_value = "true",
        action = clap::ArgAction::Set,
        value_parser = parse_tls_bool,
        value_name = "BOOL"
    )]
    pub tls_verify: bool,
}

/// Flags for the hidden `runner build` (design 010 §"runner build").
#[derive(Args)]
pub struct RunnerBuildArgs {
    /// The mounted descriptor path inside the container.
    #[arg(long, value_name = "PATH")]
    pub desc: Utf8PathBuf,

    /// Skip running the per-component build scripts.
    #[arg(long)]
    pub skip_build: bool,

    /// Rebuild even when a release/image already exists.
    #[arg(long)]
    pub force: bool,

    /// Verify registry TLS. Value-taking, as emitted by the host runner (009).
    #[arg(
        long,
        default_value = "true",
        action = clap::ArgAction::Set,
        value_parser = parse_tls_bool,
        value_name = "BOOL"
    )]
    pub tls_verify: bool,
}

/// `versions` subcommands.
#[derive(Subcommand)]
pub enum VersionsCommand {
    /// Create a new version descriptor.
    Create(VersionsCreateArgs),
}

/// Flags for `versions create` (design 010 §"versions create").
#[derive(Args)]
pub struct VersionsCreateArgs {
    /// The version this descriptor describes. Omit to auto-generate a UUIDv7.
    pub version: Option<String>,

    /// Type of version to build.
    #[arg(short = 't', long = "type", default_value = "dev")]
    pub version_type: String,

    /// A component's version, as `NAME@REF` (repeatable, required).
    #[arg(
        short = 'c',
        long = "component",
        required = true,
        value_name = "NAME@VERSION"
    )]
    pub components: Vec<String>,

    /// Directory holding component definitions (repeatable).
    #[arg(long = "components-path", value_name = "PATH")]
    pub components_paths: Vec<Utf8PathBuf>,

    /// Override a component's URI, as `COMPONENT=URI` (repeatable).
    #[arg(
        short = 'o',
        long = "override-component-uri",
        value_name = "COMPONENT=URI"
    )]
    pub override_component_uri: Vec<String>,

    /// Distribution image for this release's builder.
    #[arg(long, default_value = "rockylinux:9", value_name = "NAME")]
    pub distro: String,

    /// Distribution EL version.
    #[arg(long, default_value_t = 9, value_name = "VERSION")]
    pub el_version: u32,

    /// Registry for this release's image.
    #[arg(long, default_value = "harbor.clyso.com", value_name = "URL")]
    pub registry: String,

    /// Name for this release's image.
    #[arg(long, default_value = "ces/ceph/ceph", value_name = "NAME")]
    pub image_name: String,

    /// Tag for this release's image (defaults to the version).
    #[arg(long, value_name = "TAG")]
    pub image_tag: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // Catches conflicting flags (e.g. the root `-c` vs `versions create -c`),
        // duplicate names, and bad defaults at test time.
        Cli::command().debug_assert();
    }
}
