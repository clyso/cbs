// crt — configuration loading.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The non-secret `crt.config.yaml` (design §9): the component name and the
//! store backend (local-fs or S3). Credentials live in `crt.secrets.yaml`
//! (see `crate::secrets`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub component: String,
    pub store: StoreConfig,
}

/// The store backend. Externally tagged: `store: { local: <path> }` or
/// `store: { s3: { endpoint, region, bucket, prefix } }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreConfig {
    Local(PathBuf),
    S3(S3Config),
}

#[derive(Debug, Deserialize)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
}

/// Load and parse the config file at `path`.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    serde_yml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_local_store() {
        let cfg: Config =
            serde_yml::from_str("component: ceph\nstore:\n  local: /tmp/store\n").unwrap();
        assert_eq!(cfg.component, "ceph");
        match cfg.store {
            StoreConfig::Local(p) => assert_eq!(p, PathBuf::from("/tmp/store")),
            StoreConfig::S3(_) => panic!("expected a local store"),
        }
    }

    #[test]
    fn parses_an_s3_store() {
        let yaml = r"
component: ceph
store:
  s3:
    endpoint: https://s3.example.com
    region: us-east-1
    bucket: b
    prefix: crt/
";
        let cfg: Config = serde_yml::from_str(yaml).unwrap();
        match cfg.store {
            StoreConfig::S3(s3) => {
                assert_eq!(s3.bucket, "b");
                assert_eq!(s3.prefix, "crt/");
            }
            StoreConfig::Local(_) => panic!("expected an s3 store"),
        }
    }
}
