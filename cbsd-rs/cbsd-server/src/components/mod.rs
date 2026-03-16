// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

//! Component discovery: scans the components directory for `cbs.component.yaml`
//! files and enumerates available container versions.

pub mod tarball;

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Minimal subset of `cbs.component.yaml` needed for component discovery.
#[derive(Deserialize)]
struct ComponentYaml {
    name: String,
}

/// Information about a discovered component.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentInfo {
    pub name: String,
    pub versions: Vec<String>,
}

/// Scan `components_dir` for directories containing `cbs.component.yaml`.
/// For each, reads the `name` field and enumerates subdirectories under
/// `containers/` as version strings.
pub fn load_components(components_dir: &Path) -> Result<Vec<ComponentInfo>, std::io::Error> {
    let mut components = Vec::new();

    let entries = std::fs::read_dir(components_dir)?;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let yaml_path = entry.path().join("cbs.component.yaml");
        if !yaml_path.exists() {
            continue;
        }

        let yaml_contents = std::fs::read_to_string(&yaml_path)?;
        let parsed: ComponentYaml = serde_yml::from_str(&yaml_contents).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {e}", yaml_path.display()),
            )
        })?;

        let containers_dir = entry.path().join("containers");
        let mut versions = Vec::new();
        if containers_dir.is_dir() {
            let version_entries = std::fs::read_dir(&containers_dir)?;
            for ve in version_entries {
                let ve = ve?;
                if ve.file_type()?.is_dir() {
                    if let Some(name) = ve.file_name().to_str() {
                        versions.push(name.to_string());
                    }
                }
            }
            versions.sort();
        }

        components.push(ComponentInfo {
            name: parsed.name,
            versions,
        });
    }

    components.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(components)
}

/// Check whether `name` is a known component.
pub fn validate_component_name(components: &[ComponentInfo], name: &str) -> bool {
    components.iter().any(|c| c.name == name)
}
