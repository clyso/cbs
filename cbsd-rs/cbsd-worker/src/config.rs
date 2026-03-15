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

use std::path::PathBuf;

use serde::Deserialize;

/// Worker configuration loaded from a YAML file.
#[derive(Debug, Deserialize)]
pub struct WorkerConfig {
    /// WebSocket endpoint, e.g. `wss://cbsd.clyso.com:8080/api/ws/worker`.
    pub server_url: String,

    /// API key for authentication (`cbsk_<hex>`).
    pub api_key: String,

    /// Human-readable display label for this worker.
    pub worker_id: String,

    /// Build architecture: `x86_64` or `aarch64`.
    pub arch: String,

    /// Optional path to a custom TLS CA bundle (future enhancement).
    #[serde(default)]
    pub tls_ca_bundle_path: Option<PathBuf>,

    /// Path to the cbscore-wrapper.py script.
    #[serde(default)]
    pub cbscore_wrapper_path: Option<PathBuf>,

    /// Path to cbscore configuration file.
    #[serde(default)]
    pub cbscore_config_path: Option<PathBuf>,

    /// Build timeout in seconds.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,

    /// Temporary directory for component tarballs.
    #[serde(default)]
    pub component_temp_dir: Option<PathBuf>,

    /// Ceiling for reconnection backoff in seconds (default: 30).
    #[serde(default)]
    pub reconnect_backoff_ceiling_secs: Option<u64>,
}

impl WorkerConfig {
    /// Load configuration from a YAML file.
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Read(path.to_path_buf(), e))?;
        let config: WorkerConfig =
            serde_yml::from_str(&contents).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate required fields and value constraints.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.server_url.is_empty() {
            return Err(ConfigError::Validation(
                "server_url must not be empty".to_string(),
            ));
        }
        if self.api_key.is_empty() {
            return Err(ConfigError::Validation(
                "api_key must not be empty".to_string(),
            ));
        }
        if self.worker_id.is_empty() {
            return Err(ConfigError::Validation(
                "worker_id must not be empty".to_string(),
            ));
        }
        match self.arch.as_str() {
            "x86_64" | "aarch64" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "unsupported arch '{other}'; expected 'x86_64' or 'aarch64'"
                )));
            }
        }
        Ok(())
    }

    /// Backoff ceiling in seconds, defaulting to 30.
    pub fn backoff_ceiling_secs(&self) -> u64 {
        self.reconnect_backoff_ceiling_secs.unwrap_or(30)
    }

    /// Parse the arch string into a `cbsd_proto::Arch`.
    pub fn parsed_arch(&self) -> cbsd_proto::Arch {
        match self.arch.as_str() {
            "aarch64" => cbsd_proto::Arch::Aarch64,
            // Validated in `validate()`, so this is safe.
            _ => cbsd_proto::Arch::X86_64,
        }
    }
}

/// Errors that can occur when loading configuration.
#[derive(Debug)]
pub enum ConfigError {
    Read(PathBuf, std::io::Error),
    Parse(serde_yml::Error),
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(path, err) => {
                write!(f, "failed to read config file '{}': {err}", path.display())
            }
            Self::Parse(err) => write!(f, "failed to parse config YAML: {err}"),
            Self::Validation(msg) => write!(f, "config validation error: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(_, err) => Some(err),
            Self::Parse(err) => Some(err),
            Self::Validation(_) => None,
        }
    }
}
