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

//! The cbscore config and vault-config file formats (design 004).
//!
//! Source: `cbscore/config.py`. These are cbscore-**owned** formats, so each
//! carries a `schema-version` marker (absent → v1; design 001/002). All structs
//! are `kebab-case`, which reproduces Python's per-field aliases exactly
//! (Python aliases only the multi-word fields, and kebab-case leaves single-word
//! fields unchanged).
//!
//! These are pure types; the file `load`/`store` IO and `get_secrets` /
//! `get_vault_config` live in the `cbscore` crate.
//!
//! **Optional-field serialization.** Every unset optional is *omitted*
//! (`skip_serializing_if`), not emitted as `null`. This is a deliberate, design
//! 004–specified divergence from Python's `model_dump_json`, which emits
//! `null` for unset optionals (verified against pydantic): the port produces
//! cleaner hand-authored YAML. Round-trip is preserved either way — serde maps
//! both an absent key and an explicit `null` back to `None`.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::schema::schema_v1;

/// The main cbscore configuration (design 004; `cbscore/config.py:Config`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    /// Schema marker; absent → 1 (design 002).
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    pub paths: PathsConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing: Option<SigningConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingConfig>,
    /// Paths to the secrets files merged by `get_secrets` (later files win).
    #[serde(default)]
    pub secrets: Vec<Utf8PathBuf>,
    /// Path to the vault config file, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<Utf8PathBuf>,
}

impl Config {
    /// The highest `schema-version` this build understands.
    pub const SCHEMA_MAX: u32 = 1;
    /// Human-facing format name for schema-version errors.
    pub const SCHEMA_FORMAT: &'static str = "config";
}

/// Filesystem paths the build reads from and writes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PathsConfig {
    pub components: Vec<Utf8PathBuf>,
    pub scratch: Utf8PathBuf,
    pub scratch_containers: Utf8PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccache: Option<Utf8PathBuf>,
    // `versions` (design 006) is added in M5; not modelled here.
}

/// Storage backends; both halves optional (degrade gracefully when absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct StorageConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3StorageConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<RegistryStorageConfig>,
}

/// An S3 endpoint plus the artifact and release locations within it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct S3StorageConfig {
    pub url: String,
    pub artifacts: S3LocationConfig,
    pub releases: S3LocationConfig,
}

/// A bucket and a location prefix within it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct S3LocationConfig {
    pub bucket: String,
    pub loc: String,
}

/// Registry storage. Carried for parity but ignored by the builder today
/// (Python FIXME at `config.py:131-137`): the image's destination registry
/// comes from the version descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegistryStorageConfig {
    pub url: String,
}

/// Signing configuration; both IDs optional (resolved against the secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SigningConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transit: Option<String>,
}

/// Host-side log file location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LoggingConfig {
    pub log_file: Utf8PathBuf,
}

/// The Vault configuration (a separate file; design 004;
/// `cbscore/config.py:VaultConfig`). Auth backends are tried AppRole →
/// userpass → token at resolution time (design 004 / invariant 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct VaultConfig {
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    pub vault_addr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_user: Option<VaultUserPass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_approle: Option<VaultAppRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

impl VaultConfig {
    /// The highest `schema-version` this build understands.
    pub const SCHEMA_MAX: u32 = 1;
    /// Human-facing format name for schema-version errors.
    pub const SCHEMA_FORMAT: &'static str = "vault config";
}

/// Vault userpass credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct VaultUserPass {
    pub username: String,
    pub password: String,
}

/// Vault AppRole credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct VaultAppRole {
    pub role_id: String,
    pub secret_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_config() -> Config {
        Config {
            schema_version: 1,
            paths: PathsConfig {
                components: vec!["components".into()],
                scratch: "/scratch".into(),
                scratch_containers: "/var/lib/containers".into(),
                ccache: Some("/ccache".into()),
            },
            storage: Some(StorageConfig {
                s3: Some(S3StorageConfig {
                    url: "s3.example.com".to_string(),
                    artifacts: S3LocationConfig {
                        bucket: "art".to_string(),
                        loc: "a".to_string(),
                    },
                    releases: S3LocationConfig {
                        bucket: "rel".to_string(),
                        loc: "r".to_string(),
                    },
                }),
                registry: None,
            }),
            signing: Some(SigningConfig {
                gpg: Some("ceph".to_string()),
                transit: None,
            }),
            logging: None,
            secrets: vec!["secrets.yaml".into()],
            vault: Some("vault.yaml".into()),
        }
    }

    #[test]
    fn config_json_round_trips() {
        let cfg = full_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn kebab_aliases_and_marker_render() {
        let json = serde_json::to_value(full_config()).unwrap();
        // Multi-word fields render kebab-cased; the marker is `schema-version`.
        assert_eq!(json["schema-version"], 1);
        assert_eq!(json["paths"]["scratch-containers"], "/var/lib/containers");
    }

    #[test]
    fn unset_optionals_are_omitted_not_null() {
        // The deliberate divergence from pydantic (which emits `null`): an unset
        // optional is absent from the output entirely.
        let json = serde_json::to_value(StorageConfig {
            s3: None,
            registry: None,
        })
        .unwrap();
        let obj = json.as_object().unwrap();
        assert!(!obj.contains_key("s3"), "unset s3 must be omitted");
        assert!(
            !obj.contains_key("registry"),
            "unset registry must be omitted"
        );
    }

    #[test]
    fn absent_marker_defaults_to_v1() {
        // A marker-less config (every file written by Python) parses as v1.
        let yaml_like = r#"{"paths":{"components":["c"],"scratch":"s","scratch-containers":"sc"}}"#;
        let cfg: Config = serde_json::from_str(yaml_like).unwrap();
        assert_eq!(cfg.schema_version, 1);
        assert!(cfg.storage.is_none());
        assert!(cfg.secrets.is_empty());
    }

    #[test]
    fn vault_config_round_trips_with_approle_aliases() {
        let vc = VaultConfig {
            schema_version: 1,
            vault_addr: "https://vault.example.com".to_string(),
            auth_user: None,
            auth_approle: Some(VaultAppRole {
                role_id: "rid".to_string(),
                secret_id: "sid".to_string(),
            }),
            auth_token: None,
        };
        let json = serde_json::to_value(&vc).unwrap();
        assert_eq!(json["vault-addr"], "https://vault.example.com");
        assert_eq!(json["auth-approle"]["role-id"], "rid");
        assert_eq!(json["auth-approle"]["secret-id"], "sid");
        assert!(!json.as_object().unwrap().contains_key("auth-user"));
        let back: VaultConfig = serde_json::from_value(json).unwrap();
        assert_eq!(vc, back);
    }
}
