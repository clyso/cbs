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

//! Test-only construction helpers for `AppState` and its sub-configs.
//!
//! Handlers take `State<AppState>` as an axum extractor. Unit-testing them
//! directly (rather than via `Router::oneshot`) requires a concrete
//! `AppState` instance; the fields irrelevant to the handler under test
//! carry harmless dummy values here. Gated behind `#[cfg(test)]` so none
//! of this leaks into a release build.

#![cfg(test)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use tokio::sync::{Mutex, Notify};

use crate::app::AppState;
use crate::auth::extractors::AuthUser;
use crate::auth::oauth::OAuthState;
use crate::auth::token_cache::TokenCache;
use crate::config::{
    DevConfig, LogRetentionConfig, LoggingConfig, OAuthConfig, SecretsConfig, SeedConfig,
    ServerConfig, TimeoutsConfig,
};
use crate::logs::writer::LogWriterState;
use crate::queue::BuildQueue;

/// In-memory shared-cache SQLite pool with all migrations applied. Each
/// call produces a distinct DB namespace so parallel tests do not collide.
pub async fn test_pool() -> SqlitePool {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let url = format!(
        "file:cbsd_handler_test_{pid}_{id}?mode=memory&cache=shared",
        pid = std::process::id(),
    );
    let options = SqliteConnectOptions::from_str(&url)
        .expect("valid sqlite URL")
        .pragma("foreign_keys", "ON")
        .pragma("busy_timeout", "5000");

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .connect_with(options)
        .await
        .expect("pool");

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("migrations");

    pool
}

/// Build a fully-populated `AppState` for handler tests. The `pool` is the
/// only field most handlers actually touch; the rest carry harmless dummy
/// values (empty queue, empty worker senders, empty log indices, etc.).
pub fn test_app_state(pool: SqlitePool) -> AppState {
    AppState {
        pool,
        config: Arc::new(test_server_config()),
        oauth: OAuthState::dummy(),
        token_cache: TokenCache::new(64),
        queue: Arc::new(Mutex::new(BuildQueue::new())),
        components: Vec::new(),
        worker_senders: Arc::new(Mutex::new(HashMap::new())),
        log_watchers: Arc::new(Mutex::new(HashMap::new())),
        log_writer: Arc::new(Mutex::new(LogWriterState::new())),
        sweep_handle: Arc::new(Mutex::new(None)),
        gc_handle: Arc::new(Mutex::new(None)),
        scheduler_notify: Arc::new(Notify::new()),
        scheduler_handle: Arc::new(Mutex::new(None)),
    }
}

fn test_server_config() -> ServerConfig {
    ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        tls_cert_path: None,
        tls_key_path: None,
        db_path: ":memory:".to_string(),
        log_dir: PathBuf::from("/tmp/cbsd-test-logs"),
        components_dir: PathBuf::from("/tmp/cbsd-test-components"),
        secrets: SecretsConfig {
            token_secret_key: "0".repeat(128),
            max_token_ttl_seconds: 15_552_000,
        },
        oauth: OAuthConfig {
            secrets_file: PathBuf::from("/dev/null"),
            allowed_domains: Vec::new(),
            allow_any_google_account: true,
        },
        timeouts: TimeoutsConfig::default(),
        log_retention: LogRetentionConfig::default(),
        seed: SeedConfig::default(),
        dev: DevConfig::default(),
        logging: LoggingConfig::default(),
    }
}

/// Construct an `AuthUser` with the requested caps. Does not insert a
/// corresponding row in the DB — callers that need a real user row must
/// insert one via `db::users::create_or_update_user` or a seed helper.
pub fn auth_user(email: &str, name: &str, is_robot: bool, caps: &[&str]) -> AuthUser {
    AuthUser {
        email: email.to_string(),
        name: name.to_string(),
        caps: caps.iter().map(|s| (*s).to_string()).collect(),
        is_robot,
    }
}
