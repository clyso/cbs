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

//! CRUD management of periodic (cron-scheduled) build tasks.

use cbsd_proto::{BuildDescriptor, BuildDestImage, BuildSignedOffBy, BuildTarget};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::builds::{
    BuildDescriptorArgs, WhoamiResponse, apply_repo_overrides, format_timestamp, parse_arch,
    parse_components, parse_priority, parse_version_type,
};
use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// CLI argument types
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct PeriodicArgs {
    #[command(subcommand)]
    command: PeriodicCommands,
}

#[derive(Subcommand)]
enum PeriodicCommands {
    /// Create a new periodic build task
    New(Box<PeriodicNewArgs>),
    /// List all periodic tasks
    List,
    /// Show details of a single periodic task
    Get(PeriodicGetArgs),
    /// Update an existing periodic task
    Update(Box<PeriodicUpdateArgs>),
    /// Delete a periodic task permanently
    Delete(PeriodicDeleteArgs),
    /// Re-enable a disabled periodic task
    Enable(PeriodicEnableArgs),
    /// Disable an active periodic task
    Disable(PeriodicDisableArgs),
}

#[derive(Args)]
struct PeriodicNewArgs {
    /// Cron expression (5-field, e.g. "0 2 * * *")
    #[arg(long)]
    cron: String,

    /// Tag format with {var} placeholders.
    ///
    /// Time (UTC at trigger): {Y} {m} {d} {H} {M} {S} {DT}
    /// Descriptor: {version} {base_tag} {channel} {user} {arch} {distro} {os_version}
    #[arg(long)]
    tag_format: String,

    /// Optional description
    #[arg(long)]
    summary: Option<String>,

    /// Version string (e.g. 19.2.3)
    #[arg(long)]
    version: String,

    #[command(flatten)]
    descriptor: BuildDescriptorArgs,
}

#[derive(Args)]
struct PeriodicGetArgs {
    /// Periodic task ID
    id: String,
}

#[derive(Args)]
struct PeriodicUpdateArgs {
    /// Periodic task ID
    id: String,

    /// Cron expression (5-field)
    #[arg(long)]
    cron: Option<String>,

    /// Tag format with {var} placeholders
    #[arg(long)]
    tag_format: Option<String>,

    /// Description
    #[arg(long)]
    summary: Option<String>,

    /// Version string
    #[arg(long)]
    version: Option<String>,

    /// Release channel
    #[arg(short = 'p', long)]
    channel: Option<String>,

    /// Component in `name@gitref` format (repeat for multiple)
    #[arg(short, long = "component", num_args = 1..)]
    components: Option<Vec<String>>,

    /// Version type: release, dev, test, ci
    #[arg(short = 't', long = "type")]
    version_type: Option<String>,

    /// Base distribution
    #[arg(long)]
    distro: Option<String>,

    /// OS version string
    #[arg(long)]
    os_version: Option<String>,

    /// Destination image name
    #[arg(long)]
    image_name: Option<String>,

    /// Destination image tag
    #[arg(long)]
    image_tag: Option<String>,

    /// Build architecture: x86_64, aarch64
    #[arg(long)]
    arch: Option<String>,

    /// Repository override in `name=url` format (repeat for multiple)
    #[arg(long)]
    repo_override: Option<Vec<String>>,

    /// Build priority: high, normal, low
    #[arg(long)]
    priority: Option<String>,
}

#[derive(Args)]
struct PeriodicDeleteArgs {
    /// Periodic task ID
    id: String,
}

#[derive(Args)]
struct PeriodicEnableArgs {
    /// Periodic task ID
    id: String,
}

#[derive(Args)]
struct PeriodicDisableArgs {
    /// Periodic task ID
    id: String,
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CreatePeriodicBody {
    cron_expr: String,
    tag_format: String,
    descriptor: serde_json::Value,
    priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Deserialize)]
struct PeriodicTaskResponse {
    id: String,
    #[allow(dead_code)]
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    cron_expr: Option<String>,
    #[serde(default)]
    next_run: Option<i64>,
    #[allow(dead_code)]
    #[serde(default)]
    state: Option<String>,
}

#[derive(Deserialize)]
struct PeriodicListItem {
    id: String,
    enabled: bool,
    cron_expr: String,
    next_run: Option<i64>,
}

#[derive(Deserialize)]
struct PeriodicDetail {
    id: String,
    cron_expr: String,
    tag_format: String,
    enabled: bool,
    created_by: Option<String>,
    next_run: Option<i64>,
    retry_count: Option<i64>,
    last_error: Option<String>,
    last_build_id: Option<i64>,
    last_triggered_at: Option<i64>,
    descriptor: Option<serde_json::Value>,
    priority: Option<String>,
    summary: Option<String>,
}

#[derive(Deserialize)]
struct SimpleResponse {
    #[allow(dead_code)]
    #[serde(default)]
    detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn run(
    args: PeriodicArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    match args.command {
        PeriodicCommands::New(a) => cmd_new(*a, config_path, debug).await,
        PeriodicCommands::List => cmd_list(config_path, debug).await,
        PeriodicCommands::Get(a) => cmd_get(a, config_path, debug).await,
        PeriodicCommands::Update(a) => cmd_update(*a, config_path, debug).await,
        PeriodicCommands::Delete(a) => cmd_delete(a, config_path, debug).await,
        PeriodicCommands::Enable(a) => cmd_enable(a, config_path, debug).await,
        PeriodicCommands::Disable(a) => cmd_disable(a, config_path, debug).await,
    }
}

// ---------------------------------------------------------------------------
// periodic new
// ---------------------------------------------------------------------------

async fn cmd_new(
    args: PeriodicNewArgs,
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
        .clone()
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
            tag: image_tag,
        },
        components,
        build: BuildTarget {
            distro: args.descriptor.distro.clone(),
            os_version: args.descriptor.os_version.clone(),
            artifact_type: "rpm".to_string(),
            arch,
        },
    };

    let descriptor_json = serde_json::to_value(&descriptor)
        .map_err(|e| Error::Other(format!("cannot serialize descriptor: {e}")))?;

    let priority_str = serde_json::to_value(priority)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "normal".to_string());

    let body = CreatePeriodicBody {
        cron_expr: args.cron,
        tag_format: args.tag_format,
        descriptor: descriptor_json,
        priority: priority_str,
        summary: args.summary,
    };

    let resp: PeriodicTaskResponse = client.post("periodic", &body).await?;

    println!("periodic task {} created (enabled)", resp.id);

    if let Some(ref cron) = resp.cron_expr {
        println!("  schedule: {cron}");
    }
    if let Some(ts) = resp.next_run {
        println!("  next run: {} UTC", format_timestamp(ts));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// periodic list
// ---------------------------------------------------------------------------

async fn cmd_list(config_path: Option<&std::path::Path>, debug: bool) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let tasks: Vec<PeriodicListItem> = client.get("periodic").await?;

    if tasks.is_empty() {
        println!("no periodic tasks found");
        return Ok(());
    }

    println!(
        "  {:<10} {:<9} {:<15} NEXT RUN",
        "ID", "ENABLED", "SCHEDULE"
    );

    for task in &tasks {
        let id_short = if task.id.len() > 8 {
            &task.id[..8]
        } else {
            &task.id
        };
        let enabled = if task.enabled { "yes" } else { "no" };
        let next_run = if task.enabled {
            task.next_run
                .map(format_timestamp)
                .unwrap_or_else(|| "-".to_string())
        } else {
            "-".to_string()
        };

        println!(
            "  {:<10} {:<9} {:<15} {}",
            id_short, enabled, task.cron_expr, next_run,
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// periodic get
// ---------------------------------------------------------------------------

async fn cmd_get(
    args: PeriodicGetArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let task: PeriodicDetail = client.get(&format!("periodic/{}", args.id)).await?;

    println!("          id: {}", task.id);
    println!("        cron: {}", task.cron_expr);
    println!("  tag format: {}", task.tag_format);
    println!("     enabled: {}", if task.enabled { "yes" } else { "no" });
    if let Some(ref by) = task.created_by {
        println!("  created by: {by}");
    }
    if let Some(ts) = task.next_run {
        println!("    next run: {} UTC", format_timestamp(ts));
    } else {
        println!("    next run: -");
    }
    println!("     retries: {}", task.retry_count.unwrap_or(0));
    println!(
        "  last error: {}",
        task.last_error.as_deref().unwrap_or("-")
    );

    // Last build line: combine build ID and trigger time.
    match (task.last_build_id, task.last_triggered_at) {
        (Some(bid), Some(ts)) => {
            println!("  last build: #{bid} at {}", format_timestamp(ts));
        }
        (Some(bid), None) => {
            println!("  last build: #{bid}");
        }
        _ => {
            println!("  last build: -");
        }
    }

    // Descriptor section.
    if let Some(ref desc_val) = task.descriptor {
        println!();
        println!("  descriptor:");

        let version = desc_val
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let channel = desc_val
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let vtype = desc_val
            .get("version_type")
            .and_then(|v| v.as_str())
            .unwrap_or("-");

        let image = match (
            desc_val
                .get("dst_image")
                .and_then(|i| i.get("name"))
                .and_then(|v| v.as_str()),
            desc_val
                .get("dst_image")
                .and_then(|i| i.get("tag"))
                .and_then(|v| v.as_str()),
        ) {
            (Some(name), Some(tag)) => format!("{name}:{tag}"),
            (Some(name), None) => name.to_string(),
            _ => "-".to_string(),
        };

        let comps = desc_val
            .get("components")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let name = c.get("name")?.as_str()?;
                        let git_ref = c.get("ref")?.as_str()?;
                        Some(format!("{name}@{git_ref}"))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "-".to_string());

        let distro = desc_val
            .get("build")
            .and_then(|b| b.get("distro"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let os_ver = desc_val
            .get("build")
            .and_then(|b| b.get("os_version"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        println!("     version: {version}");
        println!("     channel: {channel}");
        println!("        type: {vtype}");
        println!("       image: {image}");
        println!("       comps: {comps}");
        println!("      distro: {distro} {os_ver}");
    }

    if let Some(ref p) = task.priority {
        println!("    priority: {p}");
    }
    if let Some(ref s) = task.summary {
        println!("     summary: {s}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// periodic update
// ---------------------------------------------------------------------------

async fn cmd_update(
    args: PeriodicUpdateArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    // Ensure at least one option is provided.
    let has_field = args.cron.is_some()
        || args.tag_format.is_some()
        || args.summary.is_some()
        || args.version.is_some()
        || args.channel.is_some()
        || args.components.is_some()
        || args.version_type.is_some()
        || args.distro.is_some()
        || args.os_version.is_some()
        || args.image_name.is_some()
        || args.image_tag.is_some()
        || args.arch.is_some()
        || args.repo_override.is_some()
        || args.priority.is_some();

    if !has_field {
        return Err(Error::Other(
            "at least one option must be provided for update".to_string(),
        ));
    }

    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    // Build a JSON object with only the provided fields.
    let mut body = serde_json::Map::new();

    if let Some(ref cron) = args.cron {
        body.insert(
            "cron_expr".to_string(),
            serde_json::Value::String(cron.clone()),
        );
    }
    if let Some(ref tag_fmt) = args.tag_format {
        body.insert(
            "tag_format".to_string(),
            serde_json::Value::String(tag_fmt.clone()),
        );
    }
    if let Some(ref summary) = args.summary {
        body.insert(
            "summary".to_string(),
            serde_json::Value::String(summary.clone()),
        );
    }
    if let Some(ref priority) = args.priority {
        // Validate the priority string.
        let _ = parse_priority(priority)?;
        body.insert(
            "priority".to_string(),
            serde_json::Value::String(priority.clone()),
        );
    }

    // If any descriptor fields are provided, construct a descriptor and include it.
    let has_descriptor_field = args.version.is_some()
        || args.channel.is_some()
        || args.components.is_some()
        || args.version_type.is_some()
        || args.distro.is_some()
        || args.os_version.is_some()
        || args.image_name.is_some()
        || args.image_tag.is_some()
        || args.arch.is_some()
        || args.repo_override.is_some();

    if has_descriptor_field {
        // Fetch existing task to merge descriptor fields.
        let existing: PeriodicDetail = client.get(&format!("periodic/{}", args.id)).await?;

        let existing_desc = existing.descriptor.unwrap_or_default();

        // Get current user for signed_off_by.
        let whoami: WhoamiResponse = client.get("auth/whoami").await?;

        // Resolve each field: use provided value or fall back to existing.
        let version = args.version.unwrap_or_else(|| {
            existing_desc
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        });

        let channel = args.channel.unwrap_or_else(|| {
            existing_desc
                .get("channel")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        });

        let version_type_str = args.version_type.unwrap_or_else(|| {
            existing_desc
                .get("version_type")
                .and_then(|v| v.as_str())
                .unwrap_or("dev")
                .to_string()
        });
        let version_type = parse_version_type(&version_type_str)?;

        let distro = args.distro.unwrap_or_else(|| {
            existing_desc
                .get("build")
                .and_then(|b| b.get("distro"))
                .and_then(|v| v.as_str())
                .unwrap_or("rockylinux")
                .to_string()
        });

        let os_version = args.os_version.unwrap_or_else(|| {
            existing_desc
                .get("build")
                .and_then(|b| b.get("os_version"))
                .and_then(|v| v.as_str())
                .unwrap_or("el9")
                .to_string()
        });

        let image_name = args.image_name.unwrap_or_else(|| {
            existing_desc
                .get("dst_image")
                .and_then(|i| i.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("ceph/ceph")
                .to_string()
        });

        let image_tag = args.image_tag.unwrap_or_else(|| {
            existing_desc
                .get("dst_image")
                .and_then(|i| i.get("tag"))
                .and_then(|v| v.as_str())
                .unwrap_or(&version)
                .to_string()
        });

        let arch_str = args.arch.unwrap_or_else(|| {
            existing_desc
                .get("build")
                .and_then(|b| b.get("arch"))
                .and_then(|v| v.as_str())
                .unwrap_or("x86_64")
                .to_string()
        });
        let arch = parse_arch(&arch_str)?;

        let raw_components: Vec<String> = args.components.unwrap_or_else(|| {
            existing_desc
                .get("components")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| {
                            let name = c.get("name")?.as_str()?;
                            let git_ref = c.get("ref")?.as_str()?;
                            Some(format!("{name}@{git_ref}"))
                        })
                        .collect()
                })
                .unwrap_or_default()
        });

        let components = parse_components(&raw_components)?;
        let repo_overrides = args.repo_override.unwrap_or_default();
        let components = apply_repo_overrides(components, &repo_overrides)?;

        let descriptor = BuildDescriptor {
            version,
            channel,
            version_type,
            signed_off_by: BuildSignedOffBy {
                user: whoami.name,
                email: whoami.email,
            },
            dst_image: BuildDestImage {
                name: image_name,
                tag: image_tag,
            },
            components,
            build: BuildTarget {
                distro,
                os_version,
                artifact_type: "rpm".to_string(),
                arch,
            },
        };

        let descriptor_json = serde_json::to_value(&descriptor)
            .map_err(|e| Error::Other(format!("cannot serialize descriptor: {e}")))?;
        body.insert("descriptor".to_string(), descriptor_json);
    }

    let _resp: SimpleResponse = client
        .put_json(
            &format!("periodic/{}", args.id),
            &serde_json::Value::Object(body),
        )
        .await?;

    println!("periodic task {} updated", args.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// periodic delete
// ---------------------------------------------------------------------------

async fn cmd_delete(
    args: PeriodicDeleteArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let _resp: SimpleResponse = client.delete(&format!("periodic/{}", args.id)).await?;
    println!("periodic task {} deleted", args.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// periodic enable
// ---------------------------------------------------------------------------

async fn cmd_enable(
    args: PeriodicEnableArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let _resp: SimpleResponse = client
        .put_empty(&format!("periodic/{}/enable", args.id))
        .await?;
    println!("periodic task {} enabled", args.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// periodic disable
// ---------------------------------------------------------------------------

async fn cmd_disable(
    args: PeriodicDisableArgs,
    config_path: Option<&std::path::Path>,
    debug: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug)?;

    let _resp: SimpleResponse = client
        .put_empty(&format!("periodic/{}/disable", args.id))
        .await?;
    println!("periodic task {} disabled", args.id);
    Ok(())
}
