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

    /// Maximum uncompressed bytes accepted from a component tarball
    /// (default: 256 MiB). Defends against gzip-bomb attacks per
    /// audit-rem D5.
    #[serde(default)]
    pub max_uncompressed_component_bytes: Option<u64>,

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

    /// When true (`CBSD_DEV` is set), TLS certificate verification is
    /// disabled so the worker can connect through reverse-proxies with
    /// self-signed certificates.
    pub dev_mode: bool,

    // Operational fields
    #[allow(dead_code)]
    pub tls_ca_bundle_path: Option<PathBuf>,
    pub cbscore_wrapper_path: Option<PathBuf>,
    pub cbscore_config_path: Option<PathBuf>,
    pub build_timeout_secs: Option<u64>,
    pub component_temp_dir: Option<PathBuf>,
    pub sigkill_escalation_timeout_secs: Option<u64>,
    pub reconnect_backoff_ceiling_secs: Option<u64>,
    /// Cap on uncompressed bytes from a component tarball (audit-rem
    /// D5). `None` → use [`crate::build::component::DEFAULT_MAX_UNCOMPRESSED_BYTES`].
    pub max_uncompressed_component_bytes: Option<u64>,
}

impl ResolvedWorkerConfig {
    /// Backoff ceiling in seconds, defaulting to 30.
    pub fn backoff_ceiling_secs(&self) -> u64 {
        self.reconnect_backoff_ceiling_secs.unwrap_or(30)
    }

    /// Resolved decompression cap for component tarballs (audit-rem D5),
    /// defaulting to [`crate::build::component::DEFAULT_MAX_UNCOMPRESSED_BYTES`].
    pub fn max_uncompressed_component_bytes(&self) -> u64 {
        self.max_uncompressed_component_bytes
            .unwrap_or(crate::build::component::DEFAULT_MAX_UNCOMPRESSED_BYTES)
    }
}

impl WorkerConfig {
    /// Load configuration from a YAML file.
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let contents =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Read(path.to_path_buf(), e))?;
        let config: WorkerConfig = serde_saphyr::from_str(&contents)
            .map_err(|e| ConfigError::Parse(path.to_path_buf(), Box::new(e)))?;
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
        let is_dev = cbsd_common::env::is_truthy_env("CBSD_DEV");
        if !is_dev && self.logging.log_file.is_none() {
            return Err(ConfigError::Validation(
                "logging.log-file is required when CBSD_DEV is not set — \
                 in production mode there is no console output, so a log \
                 file path must be configured"
                    .to_string(),
            ));
        }

        // Per audit-rem D1 (Phase 2): dev mode disables TLS certificate
        // verification (`NoVerifier`), so refuse to start when dev mode
        // is paired with a non-loopback server_url. Loopback is the only
        // safe destination for the `NoVerifier` bypass.
        if is_dev && !is_loopback_url(&self.server_url) {
            return Err(ConfigError::Validation(format!(
                "cbsd-worker refuses to start: dev mode (CBSD_DEV) is \
                 active but server_url '{}' is not a loopback address. \
                 Dev mode disables TLS certificate verification; only \
                 loopback URLs (localhost, 127.0.0.1, [::1]) are \
                 accepted in this mode.",
                self.server_url
            )));
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
            if let Some(dir) = path.parent()
                && !dir.as_os_str().is_empty()
                && !dir.exists()
            {
                return Err(ConfigError::Validation(format!(
                    "logging.log-file parent directory does not exist: '{}'",
                    dir.display()
                )));
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
                dev_mode: is_dev,
                tls_ca_bundle_path: self.tls_ca_bundle_path,
                cbscore_wrapper_path: self.cbscore_wrapper_path,
                cbscore_config_path: self.cbscore_config_path,
                build_timeout_secs: self.build_timeout_secs,
                component_temp_dir: self.component_temp_dir,
                sigkill_escalation_timeout_secs: self.sigkill_escalation_timeout_secs,
                reconnect_backoff_ceiling_secs: self.reconnect_backoff_ceiling_secs,
                max_uncompressed_component_bytes: self.max_uncompressed_component_bytes,
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
            dev_mode: is_dev,
            tls_ca_bundle_path: self.tls_ca_bundle_path,
            cbscore_wrapper_path: self.cbscore_wrapper_path,
            cbscore_config_path: self.cbscore_config_path,
            build_timeout_secs: self.build_timeout_secs,
            component_temp_dir: self.component_temp_dir,
            sigkill_escalation_timeout_secs: self.sigkill_escalation_timeout_secs,
            reconnect_backoff_ceiling_secs: self.reconnect_backoff_ceiling_secs,
            max_uncompressed_component_bytes: self.max_uncompressed_component_bytes,
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
    Parse(PathBuf, Box<serde_saphyr::Error>),
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(path, err) => {
                write!(f, "failed to read config file '{}': {err}", path.display())
            }
            Self::Parse(path, err) => {
                write!(
                    f,
                    "failed to parse config file '{}':\n{err}",
                    path.display()
                )
            }
            Self::Validation(msg) => write!(f, "config validation error: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(_, err) => Some(err),
            Self::Parse(_, err) => Some(err),
            Self::Validation(_) => None,
        }
    }
}

/// Returns `true` if `url_str` is a loopback URL: the parsed host is
/// `localhost` (ASCII-case-insensitive), an IPv4 loopback address, or
/// an IPv6 loopback address. Per WCP-style three-way match, this MUST
/// operate on the parsed `url::Host` — a `starts_with("wss://localhost")`
/// check would admit authority-confusion URLs like
/// `wss://localhost@evil.com/`.
fn is_loopback_url(url_str: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url_str) else {
        return false;
    };
    let Some(host) = parsed.host() else {
        return false;
    };
    match host {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(addr) => addr.is_loopback(),
        url::Host::Ipv6(addr) => addr.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};

    use super::*;

    /// Serializes tests that mutate the process-global `CBSD_DEV` env
    /// var. `cargo test` runs tests in this binary in parallel by
    /// default; without this lock, two tests could observe each other's
    /// env writes.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn minimal_config(server_url: &str) -> WorkerConfig {
        WorkerConfig {
            server_url: server_url.to_string(),
            worker_token: None,
            api_key: Some("legacy-api-key".to_string()),
            arch: Some("x86_64".to_string()),
            tls_ca_bundle_path: None,
            cbscore_wrapper_path: None,
            cbscore_config_path: None,
            build_timeout_secs: None,
            component_temp_dir: None,
            sigkill_escalation_timeout_secs: None,
            reconnect_backoff_ceiling_secs: None,
            max_uncompressed_component_bytes: None,
            logging: LoggingConfig::default(),
        }
    }

    #[test]
    fn resolve_with_dev_false_does_not_set_dev_mode() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);

        // Safety: env mutation is process-global. ENV_LOCK serializes
        // access; no other tests in this binary mutate CBSD_DEV.
        unsafe { std::env::set_var("CBSD_DEV", "false") };

        let mut cfg = minimal_config("wss://localhost:8443");
        // Production-style: when CBSD_DEV is not truthy, a log_file
        // path is required by resolve(). `/tmp` exists on every
        // supported test platform.
        cfg.logging.log_file = Some(PathBuf::from("/tmp/cbsd-worker-test-f1.log"));

        let resolved = cfg
            .resolve()
            .expect("resolve should succeed with CBSD_DEV=\"false\" and log_file set");
        assert!(
            !resolved.dev_mode,
            "dev_mode must be false when CBSD_DEV=\"false\""
        );

        unsafe { std::env::remove_var("CBSD_DEV") };
    }

    #[test]
    fn resolve_refuses_dev_mode_with_non_loopback_url() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);

        // Safety: see ENV_LOCK above.
        unsafe { std::env::set_var("CBSD_DEV", "1") };

        let cfg = minimal_config("wss://example.com:8443");
        let result = cfg.resolve();
        assert!(
            matches!(result, Err(ConfigError::Validation(_))),
            "expected ConfigError::Validation for dev mode with non-loopback URL"
        );

        unsafe { std::env::remove_var("CBSD_DEV") };
    }

    #[test]
    fn accepts_localhost_domain() {
        assert!(is_loopback_url("wss://localhost"));
        assert!(is_loopback_url("wss://localhost:8443/ws"));
        assert!(is_loopback_url("http://localhost/"));
    }

    #[test]
    fn accepts_localhost_case_insensitive() {
        assert!(is_loopback_url("wss://LOCALHOST"));
        assert!(is_loopback_url("wss://Localhost:8080/ws"));
    }

    #[test]
    fn accepts_ipv4_loopback() {
        assert!(is_loopback_url("wss://127.0.0.1:8443"));
        // Full `127.0.0.0/8` is loopback, not just `127.0.0.1`.
        assert!(is_loopback_url("wss://127.0.0.2"));
        assert!(is_loopback_url("wss://127.42.99.7/"));
        assert!(is_loopback_url("http://127.0.0.1"));
    }

    #[test]
    fn accepts_ipv6_loopback() {
        assert!(is_loopback_url("wss://[::1]:8443"));
        assert!(is_loopback_url("wss://[::1]/ws"));
        // IPv6 loopback with port AND path component.
        assert!(is_loopback_url("wss://[::1]:8443/x"));
    }

    #[test]
    fn rejects_non_loopback_domain() {
        assert!(!is_loopback_url("wss://example.com"));
        assert!(!is_loopback_url("wss://localhost.example.com"));
        // DNS name crafted to look like a dotted-quad: this must parse
        // as a domain, not an IPv4 literal.
        assert!(!is_loopback_url("wss://127.0.0.1.evil.com"));
    }

    #[test]
    fn rejects_authority_confusion() {
        // Per the audit-rem D1 pitfall: a naive `starts_with("wss://localhost")`
        // would accept these. The url-crate parse correctly identifies the
        // real host as `evil.com`.
        assert!(!is_loopback_url("wss://localhost@evil.com/"));
        assert!(!is_loopback_url("wss://localhost.evil.com/"));
        assert!(!is_loopback_url("wss://user:pass@evil.com/localhost"));
    }

    #[test]
    fn rejects_public_ipv4() {
        assert!(!is_loopback_url("wss://8.8.8.8"));
        assert!(!is_loopback_url("wss://192.168.1.1"));
    }

    #[test]
    fn rejects_invalid_url() {
        assert!(!is_loopback_url(""));
        assert!(!is_loopback_url("not-a-url"));
        assert!(!is_loopback_url("ws://"));
    }
}
