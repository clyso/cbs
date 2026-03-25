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

/// Web session idle timeout in seconds (7 days).
///
/// Used for the session cookie TTL after web login and as the minimum
/// allowed value for `max_token_ttl_seconds` (a token that expires
/// before the session idle timeout would produce confusing 401s on an
/// apparently live session).
pub const WEB_SESSION_IDLE_SECS: u64 = 7 * 24 * 3600;

/// Top-level server configuration. Loaded from YAML.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServerConfig {
    /// Listen address (e.g., "0.0.0.0:8080").
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// TLS certificate path (optional — if absent, plain HTTP).
    #[allow(dead_code)]
    pub tls_cert_path: Option<PathBuf>,
    /// TLS private key path.
    #[allow(dead_code)]
    pub tls_key_path: Option<PathBuf>,

    /// SQLite database file path.
    #[serde(default = "default_db_path")]
    pub db_path: String,

    /// Build log storage directory.
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,

    /// Component definitions directory (contains dirs with cbs.component.yaml).
    #[serde(default = "default_components_dir")]
    pub components_dir: PathBuf,

    /// Secrets configuration.
    pub secrets: SecretsConfig,

    /// Google OAuth configuration.
    pub oauth: OAuthConfig,

    /// Build queue timeouts.
    #[serde(default)]
    pub timeouts: TimeoutsConfig,

    /// Log retention.
    #[serde(default)]
    pub log_retention: LogRetentionConfig,

    /// First-startup seeding.
    #[serde(default)]
    pub seed: SeedConfig,

    /// Development mode configuration.
    #[serde(default)]
    pub dev: DevConfig,

    /// Tracing / logging configuration.
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SecretsConfig {
    /// 64-byte hex key for PASETO tokens and session key derivation (HKDF).
    pub token_secret_key: String,

    /// Maximum token TTL in seconds. Default: 6 months (15552000).
    #[serde(default = "default_max_token_ttl")]
    pub max_token_ttl_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct OAuthConfig {
    /// Path to Google OAuth2 secrets JSON file.
    pub secrets_file: PathBuf,

    /// Allowed email domains for Google SSO.
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Explicitly allow any Google account (must be true if allowed_domains empty).
    #[serde(default)]
    pub allow_any_google_account: bool,
}

/// Build queue and worker liveness timeouts. The parent struct uses
/// `#[serde(default)]`, so serde calls `TimeoutsConfig::default()` when the
/// entire section is missing. Per-field defaults are not needed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TimeoutsConfig {
    /// Seconds to wait for build_accepted after build_new.
    pub dispatch_ack_timeout_secs: u64,

    /// Seconds after WS drop before declaring worker dead.
    pub liveness_grace_period_secs: u64,

    /// Max backoff ceiling for worker reconnection (must be < grace period).
    pub reconnect_backoff_ceiling_secs: u64,

    /// Seconds to wait for build_finished after build_revoke.
    pub revoke_ack_timeout_secs: u64,

    /// Seconds before SIGTERM → SIGKILL escalation on worker subprocess.
    #[allow(dead_code)]
    pub sigkill_escalation_timeout_secs: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            dispatch_ack_timeout_secs: 15,
            liveness_grace_period_secs: 90,
            reconnect_backoff_ceiling_secs: 30,
            revoke_ack_timeout_secs: 30,
            sigkill_escalation_timeout_secs: 15,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LogRetentionConfig {
    /// Days to retain build log files.
    pub log_retention_days: u32,
}

impl Default for LogRetentionConfig {
    fn default() -> Self {
        Self {
            log_retention_days: 30,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SeedConfig {
    /// Admin email for first-startup seeding.
    pub seed_admin: Option<String>,
}

/// Development mode configuration. Gated by `dev.enabled` (default false).
///
/// When enabled, the server seeds workers with pre-configured API keys at
/// first startup, allowing zero-touch `podman-compose` deployments where
/// both server and worker configs reference the same key.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DevConfig {
    /// Enable development mode. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Workers to seed with pre-configured API keys on first startup.
    /// Only effective when `enabled` is true.
    #[serde(default)]
    pub seed_workers: Vec<DevSeedWorker>,
}

/// A worker to seed in dev mode, with a pre-configured API key.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DevSeedWorker {
    pub name: String,
    /// Typed as `Arch` — serde validates at parse time.
    pub arch: cbsd_proto::Arch,
    /// Pre-configured API key. Must match `cbsk_` + 64 hex chars (69 total).
    /// The same key is configured in the worker's YAML config.
    pub api_key: String,
}

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

// -- defaults for serde --

fn default_listen_addr() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_db_path() -> String {
    "cbsd.db".to_string()
}

fn default_log_dir() -> PathBuf {
    PathBuf::from("./logs")
}

fn default_components_dir() -> PathBuf {
    PathBuf::from("./components")
}

fn default_log_level() -> String {
    "info".to_string()
}

/// 6 months in seconds (180 days).
fn default_max_token_ttl() -> u64 {
    15_552_000
}

// -- validation --

impl ServerConfig {
    /// Validate configuration invariants. Panics on invalid config.
    pub fn validate(&self) {
        // Skip OAuth validation in dev mode — Google is never contacted.
        if !self.dev.enabled
            && self.oauth.allowed_domains.is_empty()
            && !self.oauth.allow_any_google_account
        {
            panic!(
                "config error: oauth.allowed_domains is empty and \
                 oauth.allow_any_google_account is not true — \
                 this would allow any Google account to authenticate"
            );
        }

        if self.timeouts.reconnect_backoff_ceiling_secs >= self.timeouts.liveness_grace_period_secs
        {
            panic!(
                "config error: reconnect_backoff_ceiling_secs ({}) must be less than \
                 liveness_grace_period_secs ({})",
                self.timeouts.reconnect_backoff_ceiling_secs,
                self.timeouts.liveness_grace_period_secs,
            );
        }

        // Token TTL must cover the web session idle timeout, otherwise
        // tokens embedded in sessions expire before the session does.
        if self.secrets.max_token_ttl_seconds < WEB_SESSION_IDLE_SECS {
            panic!(
                "config error: max-token-ttl-seconds ({}) must be >= {} \
                 (web session idle timeout is 7 days)",
                self.secrets.max_token_ttl_seconds, WEB_SESSION_IDLE_SECS,
            );
        }

        // Dev mode validation.
        if !self.dev.seed_workers.is_empty() && !self.dev.enabled {
            panic!(
                "config error: dev.seed_workers is configured but dev.enabled is false — \
                 set dev.enabled: true or remove dev.seed_workers"
            );
        }

        // Logging validation: production mode requires a log file.
        let is_dev = std::env::var("CBSD_DEV")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !is_dev && self.logging.log_file.is_none() {
            panic!(
                "config error: logging.log-file is required when CBSD_DEV is not set — \
                 in production mode there is no console output, so a log file path \
                 must be configured"
            );
        }
        if let Some(ref path) = self.logging.log_file {
            if !path.is_absolute() {
                panic!(
                    "config error: logging.log-file must be an absolute path, got '{}'",
                    path.display()
                );
            }
            if path.file_name().is_none() {
                panic!(
                    "config error: logging.log-file has no filename component: '{}'",
                    path.display()
                );
            }
            if let Some(dir) = path.parent() {
                if !dir.as_os_str().is_empty() && !dir.exists() {
                    panic!(
                        "config error: logging.log-file parent directory does not exist: '{}'",
                        dir.display()
                    );
                }
            }
        }

        for (i, w) in self.dev.seed_workers.iter().enumerate() {
            if w.api_key.len() != 69 || !w.api_key.starts_with("cbsk_") {
                panic!(
                    "config error: dev.seed_workers[{i}].api_key must be 'cbsk_' + 64 hex chars \
                     (69 characters total), got {} characters",
                    w.api_key.len()
                );
            }
            let hex_part = &w.api_key[5..];
            if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
                panic!(
                    "config error: dev.seed_workers[{i}].api_key contains non-hex characters \
                     after 'cbsk_' prefix"
                );
            }
        }
    }
}

/// Load and validate server config from a YAML file.
pub fn load_config(path: &std::path::Path) -> ServerConfig {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read config file {}: {e}", path.display()));
    let config: ServerConfig = serde_yml::from_str(&contents)
        .unwrap_or_else(|e| panic!("failed to parse config file {}: {e}", path.display()));
    config.validate();
    config
}
