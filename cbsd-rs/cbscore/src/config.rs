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

//! File IO for the config, secrets, and vault formats (design 004).
//!
//! Source: `cbscore/config.py`, `cbscore/utils/secrets/models.py`. The pure
//! types live in `cbscore-types`; this module reads and writes them. Load
//! dispatches on the file suffix — `.yaml` parses as YAML, anything else as
//! JSON, matching Python's `path.suffix.lower() == ".yaml"` test (so a `.yml`
//! file is read as JSON, a faithful quirk). `store` always writes YAML.
//!
//! YAML reading and writing use different crates by design — `serde-saphyr`
//! for reads (shared with the component loader) and `serde_yaml_ng` for writes
//! (saphyr is a deserializer only). The `store_*`/`load_*` round-trip tests
//! exercise the two together so the pair cannot silently drift.

use camino::{Utf8Path, Utf8PathBuf};
use cbscore_types::{Config, Secrets, VaultConfig};
use serde::de::DeserializeOwned;

/// An error loading or storing the config or vault config (design 004).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file '{path}' does not exist or is not a file")]
    NotFound { path: Utf8PathBuf },
    #[error("error reading config at '{path}'")]
    Read {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error parsing config at '{path}': {msg}")]
    Parse { path: Utf8PathBuf, msg: String },
    #[error(transparent)]
    Schema(cbscore_types::Error),
    #[error("error storing config to '{path}': {msg}")]
    Store { path: Utf8PathBuf, msg: String },
    #[error("error writing config to '{path}'")]
    Write {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error loading secrets from '{path}': {source}")]
    Secrets {
        path: Utf8PathBuf,
        #[source]
        source: SecretsError,
    },
    #[error("no secrets defined in config")]
    NoSecrets,
}

/// An error loading or storing a secrets file (design 004).
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("secrets file '{path}' does not exist or is not a file")]
    NotFound { path: Utf8PathBuf },
    #[error("error reading secrets at '{path}'")]
    Read {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error parsing secrets at '{path}': {msg}")]
    Parse { path: Utf8PathBuf, msg: String },
    #[error(transparent)]
    Schema(cbscore_types::Error),
    #[error("error storing secrets to '{path}': {msg}")]
    Store { path: Utf8PathBuf, msg: String },
    #[error("error writing secrets to '{path}'")]
    Write {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Outcome of the existence/readability check shared by every loader.
enum ReadErr {
    NotFound,
    Io(std::io::Error),
}

/// Read a file after the existence-and-is-a-file check Python performs.
async fn read_checked(path: &Utf8Path) -> Result<String, ReadErr> {
    match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() => tokio::fs::read_to_string(path).await.map_err(ReadErr::Io),
        _ => Err(ReadErr::NotFound),
    }
}

/// Parse `raw` as YAML when the path ends in `.yaml`, otherwise as JSON.
fn parse_by_suffix<T: DeserializeOwned>(raw: &str, path: &Utf8Path) -> Result<T, String> {
    let is_yaml = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml"));
    if is_yaml {
        serde_saphyr::from_str(raw).map_err(|e| e.to_string())
    } else {
        serde_json::from_str(raw).map_err(|e| e.to_string())
    }
}

/// Load and validate the main config from `path` (design 004; `Config.load`).
pub async fn load_config(path: &Utf8Path) -> Result<Config, ConfigError> {
    let raw = read_checked(path).await.map_err(|e| match e {
        ReadErr::NotFound => ConfigError::NotFound {
            path: path.to_owned(),
        },
        ReadErr::Io(source) => ConfigError::Read {
            path: path.to_owned(),
            source,
        },
    })?;
    let config: Config = parse_by_suffix(&raw, path).map_err(|msg| ConfigError::Parse {
        path: path.to_owned(),
        msg,
    })?;
    cbscore_types::schema::ensure_schema_version(
        Config::SCHEMA_FORMAT,
        config.schema_version,
        Config::SCHEMA_MAX,
    )
    .map_err(ConfigError::Schema)?;
    Ok(config)
}

/// Store the config to `path` as YAML (design 004; `Config.store`).
pub async fn store_config(config: &Config, path: &Utf8Path) -> Result<(), ConfigError> {
    let yaml = serde_yaml_ng::to_string(config).map_err(|e| ConfigError::Store {
        path: path.to_owned(),
        msg: e.to_string(),
    })?;
    tokio::fs::write(path, yaml)
        .await
        .map_err(|source| ConfigError::Write {
            path: path.to_owned(),
            source,
        })
}

/// Load and validate a vault config from `path` (design 004; `VaultConfig.load`
/// — note Python surfaces vault-load failures as `ConfigError`).
pub async fn load_vault(path: &Utf8Path) -> Result<VaultConfig, ConfigError> {
    let raw = read_checked(path).await.map_err(|e| match e {
        ReadErr::NotFound => ConfigError::NotFound {
            path: path.to_owned(),
        },
        ReadErr::Io(source) => ConfigError::Read {
            path: path.to_owned(),
            source,
        },
    })?;
    let vault: VaultConfig = parse_by_suffix(&raw, path).map_err(|msg| ConfigError::Parse {
        path: path.to_owned(),
        msg,
    })?;
    cbscore_types::schema::ensure_schema_version(
        VaultConfig::SCHEMA_FORMAT,
        vault.schema_version,
        VaultConfig::SCHEMA_MAX,
    )
    .map_err(ConfigError::Schema)?;
    Ok(vault)
}

/// Load and validate a secrets file from `path` (design 004; `Secrets.load`).
pub async fn load_secrets(path: &Utf8Path) -> Result<Secrets, SecretsError> {
    let raw = read_checked(path).await.map_err(|e| match e {
        ReadErr::NotFound => SecretsError::NotFound {
            path: path.to_owned(),
        },
        ReadErr::Io(source) => SecretsError::Read {
            path: path.to_owned(),
            source,
        },
    })?;
    let secrets: Secrets = parse_by_suffix(&raw, path).map_err(|msg| SecretsError::Parse {
        path: path.to_owned(),
        msg,
    })?;
    cbscore_types::schema::ensure_schema_version(
        Secrets::SCHEMA_FORMAT,
        secrets.schema_version,
        Secrets::SCHEMA_MAX,
    )
    .map_err(SecretsError::Schema)?;
    Ok(secrets)
}

/// Store secrets to `path` as YAML (design 004; `Secrets.store`). Used by the
/// runner to marshal the merged secrets into the builder container (009).
pub async fn store_secrets(secrets: &Secrets, path: &Utf8Path) -> Result<(), SecretsError> {
    let yaml = serde_yaml_ng::to_string(secrets).map_err(|e| SecretsError::Store {
        path: path.to_owned(),
        msg: e.to_string(),
    })?;
    tokio::fs::write(path, yaml)
        .await
        .map_err(|source| SecretsError::Write {
            path: path.to_owned(),
            source,
        })
}

/// Merge every configured secrets file in order (later files override earlier
/// keys; design 004; `Config.get_secrets`). Errors if the config names no
/// secrets files.
pub async fn get_secrets(config: &Config) -> Result<Secrets, ConfigError> {
    let mut merged: Option<Secrets> = None;
    for path in &config.secrets {
        let loaded = load_secrets(path)
            .await
            .map_err(|source| ConfigError::Secrets {
                path: path.clone(),
                source,
            })?;
        match merged.as_mut() {
            None => merged = Some(loaded),
            Some(acc) => acc.merge(loaded),
        }
    }
    merged.ok_or(ConfigError::NoSecrets)
}

/// Load the vault config the main config points at, if any (design 004;
/// `Config.get_vault_config`).
pub async fn get_vault_config(config: &Config) -> Result<Option<VaultConfig>, ConfigError> {
    match &config.vault {
        None => Ok(None),
        Some(path) => Ok(Some(load_vault(path).await?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbscore_types::secrets::{GitSecret, StorageSecret};

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn at(dir: &tempfile::TempDir, name: &str) -> Utf8PathBuf {
        Utf8Path::from_path(dir.path()).unwrap().join(name)
    }

    #[tokio::test]
    async fn yaml_config_round_trips_through_a_file() {
        let dir = tmpdir();
        let path = at(&dir, "cbs-build.config.yaml");
        let config = Config {
            schema_version: 1,
            paths: cbscore_types::PathsConfig {
                components: vec!["components".into()],
                scratch: "/scratch".into(),
                scratch_containers: "/var/lib/containers".into(),
                ccache: None,
            },
            storage: None,
            signing: Some(cbscore_types::SigningConfig {
                gpg: Some("ceph".to_string()),
                transit: None,
            }),
            logging: None,
            secrets: vec!["secrets.yaml".into()],
            vault: None,
        };
        store_config(&config, &path).await.unwrap();
        let back = load_config(&path).await.unwrap();
        assert_eq!(config, back);
    }

    #[tokio::test]
    async fn hand_authored_yaml_without_marker_loads_as_v1() {
        // A Python-authored config: no schema-version, optionals simply absent.
        let dir = tmpdir();
        let path = at(&dir, "config.yaml");
        tokio::fs::write(
            &path,
            "paths:\n  components: [components]\n  scratch: /scratch\n  scratch-containers: /var/lib/containers\n",
        )
        .await
        .unwrap();
        let config = load_config(&path).await.unwrap();
        assert_eq!(config.schema_version, 1);
        assert!(config.storage.is_none());
        assert_eq!(config.paths.scratch_containers, "/var/lib/containers");
    }

    #[tokio::test]
    async fn explicit_null_optional_parses_as_none() {
        // Python's model_dump_json emits `storage: null`; we must read it back.
        let dir = tmpdir();
        let path = at(&dir, "config.yaml");
        tokio::fs::write(
            &path,
            "paths:\n  components: [c]\n  scratch: /s\n  scratch-containers: /sc\nstorage: null\nvault: null\n",
        )
        .await
        .unwrap();
        let config = load_config(&path).await.unwrap();
        assert!(config.storage.is_none());
        assert!(config.vault.is_none());
    }

    #[tokio::test]
    async fn json_suffix_is_parsed_as_json() {
        let dir = tmpdir();
        let path = at(&dir, "config.json");
        tokio::fs::write(
            &path,
            r#"{"paths":{"components":["c"],"scratch":"/s","scratch-containers":"/sc"}}"#,
        )
        .await
        .unwrap();
        let config = load_config(&path).await.unwrap();
        assert_eq!(config.paths.scratch, "/s");
    }

    #[tokio::test]
    async fn higher_schema_marker_is_rejected() {
        let dir = tmpdir();
        let path = at(&dir, "config.yaml");
        tokio::fs::write(
            &path,
            "schema-version: 99\npaths:\n  components: [c]\n  scratch: /s\n  scratch-containers: /sc\n",
        )
        .await
        .unwrap();
        let err = load_config(&path).await.unwrap_err();
        assert!(matches!(err, ConfigError::Schema(_)), "{err}");
    }

    #[tokio::test]
    async fn missing_config_is_not_found() {
        let dir = tmpdir();
        let err = load_config(&at(&dir, "nope.yaml")).await.unwrap_err();
        assert!(matches!(err, ConfigError::NotFound { .. }), "{err}");
    }

    #[tokio::test]
    async fn secrets_families_discriminate_from_yaml() {
        let dir = tmpdir();
        let path = at(&dir, "secrets.yaml");
        tokio::fs::write(
            &path,
            "git:\n  github.com:\n    creds: plain\n    type: https\n    username: u\n    password: p\nstorage:\n  s3.example.com:\n    creds: vault\n    type: s3\n    key: kv/data/s3\n    access-id: AK\n    secret-id: SK\n",
        )
        .await
        .unwrap();
        let secrets = load_secrets(&path).await.unwrap();
        assert_eq!(
            secrets.git["github.com"],
            GitSecret::PlainHttps {
                username: "u".to_string(),
                password: "p".to_string(),
            }
        );
        assert_eq!(
            secrets.storage["s3.example.com"],
            StorageSecret::VaultS3 {
                key: "kv/data/s3".to_string(),
                access_id: "AK".to_string(),
                secret_id: "SK".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn get_secrets_merges_in_order() {
        let dir = tmpdir();
        let first = at(&dir, "a.yaml");
        let second = at(&dir, "b.yaml");
        tokio::fs::write(
            &first,
            "git:\n  github.com:\n    creds: plain\n    type: https\n    username: old\n    password: old\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            &second,
            "git:\n  github.com:\n    creds: plain\n    type: https\n    username: new\n    password: new\n",
        )
        .await
        .unwrap();
        let config = Config {
            schema_version: 1,
            paths: cbscore_types::PathsConfig {
                components: vec!["c".into()],
                scratch: "/s".into(),
                scratch_containers: "/sc".into(),
                ccache: None,
            },
            storage: None,
            signing: None,
            logging: None,
            secrets: vec![first, second],
            vault: None,
        };
        let merged = get_secrets(&config).await.unwrap();
        assert_eq!(
            merged.git["github.com"],
            GitSecret::PlainHttps {
                username: "new".to_string(),
                password: "new".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn get_secrets_errors_when_none_configured() {
        let config = Config {
            schema_version: 1,
            paths: cbscore_types::PathsConfig {
                components: vec!["c".into()],
                scratch: "/s".into(),
                scratch_containers: "/sc".into(),
                ccache: None,
            },
            storage: None,
            signing: None,
            logging: None,
            secrets: vec![],
            vault: None,
        };
        assert!(matches!(
            get_secrets(&config).await.unwrap_err(),
            ConfigError::NoSecrets
        ));
    }

    #[tokio::test]
    async fn stored_secrets_reload_equal() {
        let dir = tmpdir();
        let path = at(&dir, "secrets.yaml");
        let mut secrets = Secrets {
            schema_version: 1,
            git: std::collections::BTreeMap::new(),
            storage: std::collections::BTreeMap::new(),
            sign: std::collections::BTreeMap::new(),
            registry: std::collections::BTreeMap::new(),
        };
        secrets.storage.insert(
            "s3.example.com".to_string(),
            StorageSecret::PlainS3 {
                access_id: "AK".to_string(),
                secret_id: "SK".to_string(),
            },
        );
        store_secrets(&secrets, &path).await.unwrap();
        assert_eq!(load_secrets(&path).await.unwrap(), secrets);
    }
}
