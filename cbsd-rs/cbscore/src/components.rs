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

//! Component-definition loading (design 007). Source: `cbscore/core/component.py`.
//!
//! `cbs.component.yaml` is a read input authored in `components/<name>/`, so it
//! is **unversioned** (no schema marker). The full model is **strict**: `build`
//! (its rpm scripts, `get-version`, `deps`) and `containers` are required, so a
//! file missing them is rejected — restoring the validation M1's name+repo-only
//! loader relaxed (review finding F2). Both `versions create` and the builder
//! consume this one model.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tracing::{error, warn};

/// The component-definition filename inside each component directory.
const COMPONENT_FILE: &str = "cbs.component.yaml";

/// An error loading component definitions from a directory.
#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("error reading components directory '{path}'")]
    ReadDir {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The `rpm` build scripts (design 007). Optional: a component may build no RPMs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CoreComponentBuildRpm {
    pub build: String,
    pub release_rpm: String,
}

/// The `build` section: the rpm scripts plus the get-version and deps scripts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CoreComponentBuild {
    /// The rpm build scripts, absent for a component that builds no RPMs.
    /// (Python requires the `rpm` key present but nullable; serde treats a
    /// missing key as `None` too — a harmless extra leniency on this one field.)
    pub rpm: Option<CoreComponentBuildRpm>,
    pub get_version: String,
    pub deps: String,
}

/// The `containers` section: where the component's container descriptors live.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CoreComponentContainers {
    pub path: Utf8PathBuf,
}

/// A full component definition (`cbs.component.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CoreComponent {
    pub name: String,
    pub repo: String,
    pub build: CoreComponentBuild,
    pub containers: CoreComponentContainers,
}

/// A loaded component definition with the directory it came from.
#[derive(Debug, Clone)]
pub struct CoreComponentLoc {
    pub path: Utf8PathBuf,
    pub comp: CoreComponent,
}

/// Load component definitions from `paths`. Each subdirectory holding a
/// `cbs.component.yaml` yields one component, keyed by its `name`. A directory
/// without the file is skipped with a warning; a file that fails to parse or
/// **validate** (e.g. missing `build`/`containers`) is skipped with an error
/// (matching Python's `load_components`); only an unreadable directory is fatal.
pub async fn load_components(
    paths: &[Utf8PathBuf],
) -> Result<BTreeMap<String, CoreComponentLoc>, ComponentError> {
    let mut components = BTreeMap::new();
    for path in paths {
        let mut entries =
            tokio::fs::read_dir(path)
                .await
                .map_err(|source| ComponentError::ReadDir {
                    path: path.clone(),
                    source,
                })?;
        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|source| ComponentError::ReadDir {
                    path: path.clone(),
                    source,
                })?
        {
            let Ok(entry_path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue; // non-UTF-8 path; skip
            };
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }

            let comp_file = entry_path.join(COMPONENT_FILE);
            if !tokio::fs::try_exists(&comp_file).await.unwrap_or(false) {
                warn!("skipping '{entry_path}': no {COMPONENT_FILE} found");
                continue;
            }
            match load_one(&comp_file).await {
                Ok(comp) => {
                    components.insert(
                        comp.name.clone(),
                        CoreComponentLoc {
                            path: entry_path,
                            comp,
                        },
                    );
                }
                Err(e) => error!("skipping '{entry_path}': {e}"),
            }
        }
    }
    Ok(components)
}

async fn load_one(path: &Utf8Path) -> Result<CoreComponent, String> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| e.to_string())?;
    serde_saphyr::from_str::<CoreComponent>(&raw).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_YAML: &str = "name: ceph\nrepo: https://github.com/ceph/ceph\nbuild:\n  rpm:\n    build: build_rpms.sh\n    release-rpm: get_release_rpm.sh\n  get-version: get_version.sh\n  deps: install_deps.sh\ncontainers:\n  path: containers\n";

    async fn write_component(base: &Utf8Path, name: &str, yaml: &str) {
        let dir = base.join(name);
        tokio::fs::create_dir(&dir).await.unwrap();
        tokio::fs::write(dir.join(COMPONENT_FILE), yaml)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn loads_the_full_component_model() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        write_component(base, "ceph", FULL_YAML).await;

        let comps = load_components(&[base.to_owned()]).await.unwrap();
        let comp = &comps["ceph"].comp;
        assert_eq!(comp.repo, "https://github.com/ceph/ceph");
        assert_eq!(comp.build.get_version, "get_version.sh");
        assert_eq!(comp.build.deps, "install_deps.sh");
        assert_eq!(
            comp.build.rpm.as_ref().unwrap().release_rpm,
            "get_release_rpm.sh"
        );
        assert_eq!(comp.containers.path, "containers");
    }

    #[tokio::test]
    async fn a_directory_without_a_manifest_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        tokio::fs::create_dir(base.join("not-a-component"))
            .await
            .unwrap();
        write_component(base, "ceph", FULL_YAML).await;

        let comps = load_components(&[base.to_owned()]).await.unwrap();
        assert_eq!(comps.len(), 1);
    }

    #[tokio::test]
    async fn a_manifest_missing_build_or_containers_is_rejected() {
        // The F2 fix: the lenient name+repo-only file Python rejects is now
        // error-skipped, not loaded.
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        write_component(
            base,
            "ceph",
            "name: ceph\nrepo: https://github.com/ceph/ceph\n",
        )
        .await;

        let comps = load_components(&[base.to_owned()]).await.unwrap();
        assert!(comps.is_empty(), "incomplete manifest must be rejected");
    }

    #[tokio::test]
    async fn the_rpm_section_is_optional() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        write_component(
            base,
            "tools",
            "name: tools\nrepo: https://example.com/tools\nbuild:\n  get-version: gv.sh\n  deps: deps.sh\ncontainers:\n  path: c\n",
        )
        .await;

        let comps = load_components(&[base.to_owned()]).await.unwrap();
        assert!(comps["tools"].comp.build.rpm.is_none());
    }
}
