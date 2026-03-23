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

//! Build submission, listing, revocation, and component discovery.

use cbsd_proto::{
    Arch, BuildComponent, BuildDescriptor, BuildDestImage, BuildSignedOffBy, BuildTarget, Priority,
    VersionType,
};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;
use crate::logs::LogsArgs;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct BuildArgs {
    #[command(subcommand)]
    command: BuildCommands,
}

#[derive(Subcommand)]
enum BuildCommands {
    /// Submit a new build
    New(Box<BuildNewArgs>),
    /// List builds
    List(BuildListArgs),
    /// Show details of a single build
    Get(BuildGetArgs),
    /// Cancel a build
    Revoke(BuildRevokeArgs),
    /// List available build components
    Components,
    /// View build logs (tail, follow, download)
    Logs(LogsArgs),
}

/// Options for constructing a `BuildDescriptor`.
///
/// Shared between `build new` (positional VERSION) and `periodic new`
/// (named `--version`). Does NOT include the version field.
#[derive(Args, Clone)]
pub struct BuildDescriptorArgs {
    /// Release channel
    #[arg(short = 'p', long)]
    pub channel: String,

    /// Component in `name@gitref` format (repeat for multiple)
    #[arg(short, long = "component", num_args = 1..)]
    pub components: Vec<String>,

    /// Version type: release, dev, test, ci
    #[arg(short = 't', long = "type", default_value = "dev")]
    pub version_type: String,

    /// Base distribution
    #[arg(long, default_value = "rockylinux")]
    pub distro: String,

    /// OS version string
    #[arg(long, default_value = "el9")]
    pub os_version: String,

    /// Destination image name
    #[arg(long, default_value = "ceph/ceph")]
    pub image_name: String,

    /// Destination image tag (defaults to VERSION)
    #[arg(long)]
    pub image_tag: Option<String>,

    /// Build architecture: x86_64, aarch64
    #[arg(long, default_value = "x86_64")]
    pub arch: String,

    /// Repository override in `name=url` format (repeat for multiple)
    #[arg(long)]
    pub repo_override: Vec<String>,

    /// Build priority: high, normal, low
    #[arg(long, default_value = "normal")]
    pub priority: String,
}

#[derive(Args)]
struct BuildNewArgs {
    /// Version string (e.g. 19.2.3)
    version: String,

    #[command(flatten)]
    descriptor: BuildDescriptorArgs,
}

#[derive(Args)]
struct BuildListArgs {
    /// Show all users' builds (not just your own)
    #[arg(long)]
    all: bool,

    /// Filter by build state
    #[arg(long)]
    state: Option<String>,

    /// Maximum number of builds to display
    #[arg(short = 'n', long, default_value = "20")]
    limit: usize,
}

#[derive(Args)]
struct BuildGetArgs {
    /// Build ID
    id: i64,

    /// Output as compact JSON for tool consumption
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct BuildRevokeArgs {
    /// Build ID
    id: i64,
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SubmitBuildBody {
    descriptor: BuildDescriptor,
    priority: Priority,
}

#[derive(Deserialize)]
struct SubmitBuildResponse {
    id: i64,
    state: String,
    warning: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct BuildRecord {
    id: i64,
    descriptor: String,
    user_email: String,
    priority: String,
    state: String,
    worker_id: Option<String>,
    trace_id: Option<String>,
    error: Option<String>,
    submitted_at: i64,
    queued_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    #[serde(default)]
    build_report: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct WhoamiResponse {
    pub email: String,
    pub name: String,
}

#[derive(Deserialize)]
struct ComponentInfo {
    name: String,
    versions: Vec<String>,
}

#[derive(Deserialize)]
struct RevokeResponse {
    detail: String,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: BuildArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    match args.command {
        BuildCommands::New(a) => cmd_new(*a, config_path, debug).await,
        BuildCommands::List(a) => cmd_list(a, config_path, debug).await,
        BuildCommands::Get(a) => cmd_get(a, config_path, debug).await,
        BuildCommands::Revoke(a) => cmd_revoke(a, config_path, debug).await,
        BuildCommands::Components => cmd_components(config_path, debug).await,
        BuildCommands::Logs(a) => crate::logs::run(a, config_path, debug).await,
    }
}

// ---------------------------------------------------------------------------
// build new
// ---------------------------------------------------------------------------

async fn cmd_new(
    args: BuildNewArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    // Get current user for signed_off_by.
    let whoami: WhoamiResponse = client.get("auth/whoami").await?;

    // Parse --component values: "name@gitref".
    let components = parse_components(&args.descriptor.components)?;

    // Parse --repo-override values: "name=url". Apply to matching components.
    let components = apply_repo_overrides(components, &args.descriptor.repo_override)?;

    // Parse --type into VersionType.
    let version_type = parse_version_type(&args.descriptor.version_type)?;

    // Parse --arch into Arch.
    let arch = parse_arch(&args.descriptor.arch)?;

    // Parse --priority into Priority.
    let priority = parse_priority(&args.descriptor.priority)?;

    let image_tag = args
        .descriptor
        .image_tag
        .unwrap_or_else(|| args.version.clone());

    let descriptor = BuildDescriptor {
        version: args.version.clone(),
        channel: args.descriptor.channel.clone(),
        version_type,
        signed_off_by: BuildSignedOffBy {
            user: whoami.name,
            email: whoami.email,
        },
        dst_image: BuildDestImage {
            name: args.descriptor.image_name.clone(),
            tag: image_tag.clone(),
        },
        components: components.clone(),
        build: BuildTarget {
            distro: args.descriptor.distro.clone(),
            os_version: args.descriptor.os_version.clone(),
            artifact_type: "rpm".to_string(),
            arch,
        },
    };

    let body = SubmitBuildBody {
        descriptor,
        priority,
    };

    let resp: SubmitBuildResponse = client.post("builds", &body).await?;

    // Echo the locally-constructed descriptor summary.
    let comps_display: Vec<String> = components
        .iter()
        .map(|c| format!("{}@{}", c.name, c.git_ref))
        .collect();

    println!(
        "  version: {}\n  channel: {}\n     type: {}\n    image: {}:{}\n    comps: {}\n   distro: {} {}",
        args.version,
        args.descriptor.channel,
        args.descriptor.version_type,
        args.descriptor.image_name,
        image_tag,
        comps_display.join(", "),
        args.descriptor.distro,
        args.descriptor.os_version,
    );
    println!();
    println!("build {} submitted ({})", resp.id, resp.state);

    if let Some(warning) = resp.warning {
        println!("warning: {warning}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// build list
// ---------------------------------------------------------------------------

async fn cmd_list(
    args: BuildListArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    // Build query path.
    let mut query_parts: Vec<String> = Vec::new();

    if !args.all {
        let whoami: WhoamiResponse = client.get("auth/whoami").await?;
        query_parts.push(format!("user={}", whoami.email));
    }

    if let Some(ref state) = args.state {
        query_parts.push(format!("state={state}"));
    }

    let path = if query_parts.is_empty() {
        "builds".to_string()
    } else {
        format!("builds?{}", query_parts.join("&"))
    };

    let mut builds: Vec<BuildRecord> = client.get(&path).await?;

    // Sort by ID descending (newest first).
    builds.sort_by(|a, b| b.id.cmp(&a.id));

    // Client-side truncation.
    builds.truncate(args.limit);

    if builds.is_empty() {
        println!("no builds found");
        return Ok(());
    }

    for build in &builds {
        let ts = format_timestamp(build.submitted_at);
        println!(
            "  #{:<5} {:<12} {:<30} {}",
            build.id, build.state, build.user_email, ts,
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// build get
// ---------------------------------------------------------------------------

async fn cmd_get(
    args: BuildGetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let build: BuildRecord = client.get(&format!("builds/{}", args.id)).await?;

    // --json: compact JSON with {info, report} structure.
    if args.json {
        let desc: Option<serde_json::Value> =
            serde_json::from_str(&build.descriptor).ok();
        let output = serde_json::json!({
            "info": {
                "id": build.id,
                "state": build.state,
                "user_email": build.user_email,
                "priority": build.priority,
                "descriptor": desc,
                "worker_id": build.worker_id,
                "trace_id": build.trace_id,
                "error": build.error,
                "submitted_at": build.submitted_at,
                "queued_at": build.queued_at,
                "started_at": build.started_at,
                "finished_at": build.finished_at,
            },
            "report": build.build_report,
        });
        println!(
            "{}",
            serde_json::to_string(&output)
                .map_err(|e| Error::Other(format!("JSON serialization error: {e}")))?
        );
        return Ok(());
    }

    // Human-readable output.
    let desc: Option<BuildDescriptor> = serde_json::from_str(&build.descriptor).ok();

    println!("      id: {}", build.id);
    println!("   state: {}", build.state);
    println!("    user: {}", build.user_email);
    println!("priority: {}", build.priority);

    if let Some(ref d) = desc {
        println!(" version: {}", d.version);
        println!(" channel: {}", d.channel);
        println!(
            "    type: {}",
            serde_json::to_string(&d.version_type)
                .unwrap_or_default()
                .trim_matches('"')
        );

        println!("   image: {}:{}", d.dst_image.name, d.dst_image.tag);

        let comps: Vec<String> = d
            .components
            .iter()
            .map(|c| format!("{}@{}", c.name, c.git_ref))
            .collect();
        println!("   comps: {}", comps.join(", "));
        println!("  distro: {} {}", d.build.distro, d.build.os_version);
    }

    if let Some(ref wid) = build.worker_id {
        println!("  worker: {wid}");
    }
    if let Some(ref tid) = build.trace_id {
        println!("   trace: {tid}");
    }

    println!("  queued: {}", format_timestamp(build.submitted_at));
    if let Some(ts) = build.started_at {
        println!(" started: {}", format_timestamp(ts));
    }
    if let Some(ts) = build.finished_at {
        println!("finished: {}", format_timestamp(ts));
    }

    // Artifact report (only for completed builds with a report).
    if let Some(ref report) = build.build_report {
        println!();
        println!("  report:");

        if let Some(img) = report.get("container_image") {
            let name = img.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let tag = img.get("tag").and_then(|v| v.as_str()).unwrap_or("-");
            let pushed = img.get("pushed").and_then(|v| v.as_bool()).unwrap_or(false);
            let pushed_str = if pushed { "pushed" } else { "not pushed" };
            println!("    image: {name}:{tag} ({pushed_str})");
        }

        if let Some(comps) = report.get("components").and_then(|v| v.as_array()) {
            if !comps.is_empty() {
                println!("    components:");
                for c in comps {
                    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                    let ver = c.get("version").and_then(|v| v.as_str()).unwrap_or("-");
                    let sha = c.get("sha1").and_then(|v| v.as_str()).unwrap_or("-");
                    let sha_short = if sha.len() > 7 { &sha[..7] } else { sha };
                    println!("      {name} {ver} ({sha_short})");

                    if let Some(rpms) = c.get("rpms_s3_path").and_then(|v| v.as_str()) {
                        println!("        rpms: {rpms}");
                    }
                }
            }
        }

        if let Some(rel) = report.get("release_descriptor") {
            let bucket = rel.get("bucket").and_then(|v| v.as_str()).unwrap_or("-");
            let path = rel.get("s3_path").and_then(|v| v.as_str()).unwrap_or("-");
            println!("    release: s3://{bucket}/{path}");
        }

        if report.get("skipped").and_then(|v| v.as_bool()).unwrap_or(false) {
            println!("    (skipped — image already existed)");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// build revoke
// ---------------------------------------------------------------------------

async fn cmd_revoke(
    args: BuildRevokeArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let resp: RevokeResponse = client.delete(&format!("builds/{}", args.id)).await?;
    println!("{}", resp.detail);

    Ok(())
}

// ---------------------------------------------------------------------------
// build components
// ---------------------------------------------------------------------------

async fn cmd_components(config_path: Option<&std::path::Path>, debug: bool) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let components: Vec<ComponentInfo> = client.get("components").await?;

    if components.is_empty() {
        println!("no components found");
        return Ok(());
    }

    for comp in &components {
        println!("  {}", comp.name);
        println!("    versions: {}", comp.versions.join(", "));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse `--component` values: split on `@` to get `{name, git_ref}`.
pub fn parse_components(raw: &[String]) -> Result<Vec<BuildComponent>, Error> {
    raw.iter()
        .map(|s| {
            let (name, git_ref) = s.split_once('@').ok_or_else(|| {
                Error::Other(format!("invalid component '{s}': expected name@ref"))
            })?;
            Ok(BuildComponent {
                name: name.to_string(),
                git_ref: git_ref.to_string(),
                repo: None,
            })
        })
        .collect()
}

/// Parse `--repo-override` values: split on first `=` to get `{name, url}`.
/// Match overrides to components by name and set the `repo` field.
pub fn apply_repo_overrides(
    mut components: Vec<BuildComponent>,
    overrides: &[String],
) -> Result<Vec<BuildComponent>, Error> {
    for ov in overrides {
        let (name, url) = ov.split_once('=').ok_or_else(|| {
            Error::Other(format!("invalid repo override '{ov}': expected name=url"))
        })?;
        let comp = components
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::Other(format!("repo override for unknown component '{name}'")))?;
        comp.repo = Some(url.to_string());
    }
    Ok(components)
}

/// Parse a version type string into `VersionType`.
pub fn parse_version_type(s: &str) -> Result<VersionType, Error> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
        Error::Other(format!(
            "invalid version type '{s}': expected release, dev, test, or ci"
        ))
    })
}

/// Parse an architecture string into `Arch`.
pub fn parse_arch(s: &str) -> Result<Arch, Error> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
        Error::Other(format!(
            "invalid architecture '{s}': expected x86_64 or aarch64"
        ))
    })
}

/// Parse a priority string into `Priority`.
pub fn parse_priority(s: &str) -> Result<Priority, Error> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
        Error::Other(format!(
            "invalid priority '{s}': expected high, normal, or low"
        ))
    })
}

/// Format a Unix timestamp as a human-readable date/time string.
pub fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}
