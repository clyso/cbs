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

//! The `versions create` handler (design 010). `versions create` never loads
//! the config; it takes CLI flags and the local git repo. The trailing
//! "image descriptor missing?" note (`get_image_desc`) lands in the next commit.

use std::process::ExitCode;

use cbscore::images::desc::{ImageError, get_image_desc};
use cbscore::utils::git::{get_git_repo_root, get_git_user};
use cbscore::versions::{
    get_version_type, parse_component_refs, parse_version, resolve_version, version_create_helper,
    write_descriptor,
};

use crate::cli::VersionsCreateArgs;

/// Handle `cbsbuild versions create`. Returns `0` on success and `1` on a
/// runtime failure (clap already handles usage errors with exit `2`).
pub async fn create(args: &VersionsCreateArgs) -> ExitCode {
    let version_type = match get_version_type(&args.version_type) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Omitted VERSION → a fresh sortable UUIDv7.
    let version = resolve_version(args.version.as_deref());

    let (user_name, user_email) = match get_git_user().await {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error obtaining git user info: {e}");
            return ExitCode::FAILURE;
        }
    };

    let component_refs = match parse_component_refs(&args.components) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error parsing component refs: {e}");
            return ExitCode::FAILURE;
        }
    };

    let spec = cbscore::versions::VersionSpec {
        version: &version,
        version_type,
        component_refs: &component_refs,
        distro: &args.distro,
        el_version: args.el_version,
        registry: &args.registry,
        image_name: &args.image_name,
        image_tag: args.image_tag.as_deref(),
        user_name: &user_name,
        user_email: &user_email,
    };
    let desc =
        match version_create_helper(&spec, &args.components_paths, &args.override_component_uri)
            .await
        {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error creating version descriptor: {e}");
                return ExitCode::FAILURE;
            }
        };

    println!("version: {}", desc.version);
    println!("version title: {}", desc.title);

    let repo_root = match get_git_repo_root().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error obtaining git repo root: {e}");
            return ExitCode::FAILURE;
        }
    };

    let store_root = repo_root.join("_versions");
    let path = match write_descriptor(&desc, &store_root, version_type).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if let Ok(json) = desc.to_json_pretty() {
        println!("{json}");
    }
    println!("-> written to {path}");

    // Trailing, non-fatal note: is there an image descriptor for this version?
    // Skipped for versions without an M.m.p (UUIDv7 / patch-less), which cannot
    // key an image descriptor (design 006).
    let has_mmp = parse_version(&desc.version)
        .map(|p| p.minor.is_some() && p.patch.is_some())
        .unwrap_or(false);
    if has_mmp {
        match get_image_desc(&repo_root, &desc.version).await {
            Ok(_) => {}
            Err(ImageError::NoSuchVersion(_)) => {
                println!("image descriptor for version '{}' missing", desc.version);
            }
            Err(e) => {
                eprintln!(
                    "error obtaining image descriptor for '{}': {e}",
                    desc.version
                );
            }
        }
    }

    ExitCode::SUCCESS
}
