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

//! Minimal component-definition loading for `versions create` (design 006).
//!
//! Source: `cbscore/core/component.py`. `versions create` needs only each
//! component's `name` and `repo`; the full `CoreComponent` model (the
//! `build`/`containers` sections used by the builder pipeline) is owned by 007
//! and lands with component preparation (C3). Extra keys in the YAML are ignored
//! here, so this reads the same `cbs.component.yaml` files unchanged.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tracing::warn;

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

/// A component definition reduced to what `versions create` needs.
#[derive(Debug, Clone, Deserialize)]
pub struct ComponentDef {
    pub name: String,
    pub repo: String,
}

/// A loaded component definition with the directory it came from.
#[derive(Debug, Clone)]
pub struct ComponentLoc {
    pub path: Utf8PathBuf,
    pub def: ComponentDef,
}

/// Load component definitions from `paths`. Each subdirectory holding a
/// `cbs.component.yaml` yields one component, keyed by its `name`. A directory
/// without the file, or a file that fails to parse, is skipped with a warning
/// (matching Python's leniency); only an unreadable directory is a hard error.
pub async fn load_components(
    paths: &[Utf8PathBuf],
) -> Result<BTreeMap<String, ComponentLoc>, ComponentError> {
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
                Ok(def) => {
                    components.insert(
                        def.name.clone(),
                        ComponentLoc {
                            path: entry_path,
                            def,
                        },
                    );
                }
                Err(e) => warn!("skipping '{entry_path}': {e}"),
            }
        }
    }
    Ok(components)
}

async fn load_one(path: &Utf8Path) -> Result<ComponentDef, String> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| e.to_string())?;
    serde_saphyr::from_str::<ComponentDef>(&raw).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_name_and_repo_ignoring_extra_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();

        let ceph_dir = base.join("ceph");
        tokio::fs::create_dir(&ceph_dir).await.unwrap();
        // A real cbs.component.yaml carries build/containers too; we ignore them.
        tokio::fs::write(
            ceph_dir.join(COMPONENT_FILE),
            "name: ceph\nrepo: https://github.com/ceph/ceph\nbuild:\n  rpm:\n    build: b.sh\n    release-rpm: r.sh\n  get-version: gv.sh\n  deps: deps.sh\ncontainers:\n  path: containers\n",
        )
        .await
        .unwrap();

        // A directory without the file is skipped.
        tokio::fs::create_dir(base.join("not-a-component"))
            .await
            .unwrap();

        let comps = load_components(&[base.to_owned()]).await.unwrap();
        assert_eq!(comps.len(), 1);
        assert_eq!(comps["ceph"].def.repo, "https://github.com/ceph/ceph");
        assert_eq!(comps["ceph"].path, ceph_dir);
    }
}
