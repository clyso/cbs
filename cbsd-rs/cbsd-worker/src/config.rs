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

use base64::Engine;
use serde::Deserialize;

/// Logging configuration for the worker binary.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LoggingConfig {
    /// Log level (e.g., "info", "debug", "trace").
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log file path. Required when `CBSD_DEV` is not set (production).
    pub log_file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            log_file: None,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

/// Worker configuration loaded from a YAML file.
///
/// Identity fields (`api_key`, `arch`) can be provided individually or
/// via a `worker_token` (base64url-encoded JSON from the registration API).
/// The `CBSD_WORKER_TOKEN` env var takes highest precedence.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WorkerConfig {
    /// WebSocket endpoint, e.g. `wss://cbsd.clyso.com:8080/api/ws/worker`.
    pub server_url: String,

    /// Base64url-encoded worker token from `POST /api/admin/workers`.
    /// Overrides `api_key` and `arch` when present.
    #[serde(default)]
    pub worker_token: Option<String>,

    /// API key for authentication (`cbsk_<hex>`). Used when no token.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Build architecture: `x86_64` or `aarch64`. Used when no token.
    #[serde(default)]
    pub arch: Option<String>,

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

    /// Seconds before SIGTERM → SIGKILL escalation on build subprocess.
    #[serde(default)]
    pub sigkill_escalation_timeout_secs: Option<u64>,

    /// Ceiling for reconnection backoff in seconds (default: 30).
    #[serde(default)]
    pub reconnect_backoff_ceiling_secs: Option<u64>,

    /// Logging configuration.
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Resolved worker configuration with all identity fields guaranteed present.
pub struct ResolvedWorkerConfig {
    pub server_url: String,
    pub api_key: String,
    /// For local logging only — not sent over the wire.
    pub worker_name: String,
    pub arch: cbsd_proto::Arch,

    // Operational fields
    #[allow(dead_code)]
    pub tls_ca_bundle_path: Option<PathBuf>,
    pub cbscore_wrapper_path: Option<PathBuf>,
    pub cbscore_config_path: Option<PathBuf>,
    pub build_timeout_secs: Option<u64>,
    pub component_temp_dir: Option<PathBuf>,
    pub sigkill_escalation_timeout_secs: Option<u64>,
    pub reconnect_backoff_ceiling_secs: Option<u64>,
}

impl ResolvedWorkerConfig {
    /// Backoff ceiling in seconds, defaulting to 30.
    pub fn backoff_ceiling_secs(&self) -> u64 {
        self.reconnect_backoff_ceiling_secs.unwrap_or(30)
    }
}

impl WorkerConfig {
    /// Load configuration from a YAML file.
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let contents =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Read(path.to_path_buf(), e))?;
        let config: WorkerConfig = serde_yml::from_str(&contents).map_err(ConfigError::Parse)?;
        Ok(config)
    }

    /// Resolve identity fields from token or individual fields.
    ///
    /// Priority: `CBSD_WORKER_TOKEN` env var > `worker_token` config field >
    /// individual `api_key` + `arch` fields.
    pub fn resolve(self) -> Result<ResolvedWorkerConfig, ConfigError> {
        if self.server_url.is_empty() {
            return Err(ConfigError::Validation(
                "server_url must not be empty".to_string(),
            ));
        }

        // Logging validation: production mode requires a log file.
        let is_dev = std::env::var("CBSD_DEV")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !is_dev && self.logging.log_file.is_none() {
            return Err(ConfigError::Validation(
                "logging.log-file is required when CBSD_DEV is not set — \
                 in production mode there is no console output, so a log \
                 file path must be configured"
                    .to_string(),
            ));
        }
        if let Some(ref path) = self.logging.log_file {
            if !path.is_absolute() {
                return Err(ConfigError::Validation(format!(
                    "logging.log-file must be an absolute path, got '{}'",
                    path.display()
                )));
            }
            if path.file_name().is_none() {
                return Err(ConfigError::Validation(format!(
                    "logging.log-file has no filename component: '{}'",
                    path.display()
                )));
            }
            if let Some(dir) = path.parent() {
                if !dir.as_os_str().is_empty() && !dir.exists() {
                    return Err(ConfigError::Validation(format!(
                        "logging.log-file parent directory does not exist: '{}'",
                        dir.display()
                    )));
                }
            }
        }

        // Try env var first
        let token_str = std::env::var("CBSD_WORKER_TOKEN").ok();

        if token_str.is_some() && self.worker_token.is_some() {
            tracing::warn!(
                "both CBSD_WORKER_TOKEN env var and worker_token config are set — \
                 env var takes precedence"
            );
        }

        let token_b64 = token_str.or(self.worker_token);

        if let Some(b64) = token_b64 {
            let json_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(b64.as_bytes())
                .map_err(|e| {
                    ConfigError::Validation(format!("invalid worker token base64: {e}"))
                })?;
            let token: cbsd_proto::WorkerToken = serde_json::from_slice(&json_bytes)
                .map_err(|e| ConfigError::Validation(format!("invalid worker token JSON: {e}")))?;

            let arch = parse_arch(&token.arch)?;

            return Ok(ResolvedWorkerConfig {
                server_url: self.server_url,
                api_key: token.api_key,
                worker_name: token.worker_name,
                arch,
                tls_ca_bundle_path: self.tls_ca_bundle_path,
                cbscore_wrapper_path: self.cbscore_wrapper_path,
                cbscore_config_path: self.cbscore_config_path,
                build_timeout_secs: self.build_timeout_secs,
                component_temp_dir: self.component_temp_dir,
                sigkill_escalation_timeout_secs: self.sigkill_escalation_timeout_secs,
                reconnect_backoff_ceiling_secs: self.reconnect_backoff_ceiling_secs,
            });
        }

        // Legacy mode: individual fields
        let api_key = self.api_key.ok_or_else(|| {
            ConfigError::Validation(
                "api_key is required when no worker_token is provided".to_string(),
            )
        })?;
        if api_key.is_empty() {
            return Err(ConfigError::Validation(
                "api_key must not be empty".to_string(),
            ));
        }

        let arch_str = self.arch.ok_or_else(|| {
            ConfigError::Validation("arch is required when no worker_token is provided".to_string())
        })?;
        let arch = parse_arch(&arch_str)?;

        // In legacy mode, worker_name is for local logging only.
        let worker_name = "legacy-worker".to_string();

        Ok(ResolvedWorkerConfig {
            server_url: self.server_url,
            api_key,
            worker_name,
            arch,
            tls_ca_bundle_path: self.tls_ca_bundle_path,
            cbscore_wrapper_path: self.cbscore_wrapper_path,
            cbscore_config_path: self.cbscore_config_path,
            build_timeout_secs: self.build_timeout_secs,
            component_temp_dir: self.component_temp_dir,
            sigkill_escalation_timeout_secs: self.sigkill_escalation_timeout_secs,
            reconnect_backoff_ceiling_secs: self.reconnect_backoff_ceiling_secs,
        })
    }
}

fn parse_arch(s: &str) -> Result<cbsd_proto::Arch, ConfigError> {
    match s {
        "x86_64" => Ok(cbsd_proto::Arch::X86_64),
        "aarch64" | "arm64" => Ok(cbsd_proto::Arch::Aarch64),
        other => Err(ConfigError::Validation(format!(
            "unsupported arch '{other}'; expected 'x86_64' or 'aarch64'"
        ))),
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
