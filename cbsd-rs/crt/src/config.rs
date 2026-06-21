// crt — configuration loading.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The non-secret `crt.config.yaml` (design §9). In M1 this carries the
//! component name and a local-filesystem store root; S3 + the secrets file
//! arrive in commit 1.2.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub component: String,
    pub store: StoreConfig,
}

#[derive(Debug, Deserialize)]
pub struct StoreConfig {
    /// Local-filesystem store root. (S3 is added in commit 1.2.)
    pub local: PathBuf,
}

/// Load and parse the config file at `path`.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    serde_yml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}
