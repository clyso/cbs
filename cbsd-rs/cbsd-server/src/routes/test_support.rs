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
use tower_sessions::SessionManagerLayer;
use tower_sessions::service::SignedCookie;
use tower_sessions_sqlx_store::SqliteStore;

use crate::app::AppState;
use crate::auth::extractors::AuthUser;
use crate::auth::oauth::OAuthState;
use crate::auth::token_cache::TokenCache;
use crate::config::{
    DevConfig, LogRetentionConfig, LoggingConfig, MetricsConfig, OAuthConfig, SecretsConfig,
    SeedConfig, ServerConfig, TimeoutsConfig,
};
use crate::logs::writer::LogWriterState;
use crate::queue::BuildQueue;

/// PASETO v4 symmetric key used by every test `ServerConfig` and by
/// [`seed_authed_bearer`] — a valid 32-byte hex key so tests can mint
/// working bearer tokens against the test config.
pub fn test_token_secret() -> String {
    "0".repeat(64)
}

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
    test_app_state_with_components_dir(pool, PathBuf::from("/tmp/cbsd-test-components"))
}

/// `test_app_state` variant that lets the caller override the
/// `components_dir`. Required by dispatch-level tests that pack a real
/// component tarball from a tempdir.
pub fn test_app_state_with_components_dir(pool: SqlitePool, components_dir: PathBuf) -> AppState {
    AppState {
        pool,
        config: Arc::new(test_server_config_with_components_dir(components_dir)),
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
        metrics: None,
    }
}

fn test_server_config_with_components_dir(components_dir: PathBuf) -> ServerConfig {
    ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        tls_cert_path: None,
        tls_key_path: None,
        db_path: ":memory:".to_string(),
        log_dir: PathBuf::from("/tmp/cbsd-test-logs"),
        components_dir,
        secrets: SecretsConfig {
            token_secret_key: test_token_secret(),
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
        metrics: MetricsConfig::default(),
    }
}

/// Create a tempdir with a single child directory named `component_name`
/// holding one placeholder file, structured so that
/// `cbsd-server::components::tarball::pack_component(&tempdir.path(),
/// component_name)` succeeds. The returned `TempDir` cleans up on drop;
/// callers must keep it alive for the duration of the test.
pub fn temp_component_dir(component_name: &str) -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let component_root = tmp.path().join(component_name);
    std::fs::create_dir_all(&component_root).expect("mkdir component");
    std::fs::write(component_root.join("placeholder"), b"test\n").expect("write placeholder");
    tmp
}

/// Like [`temp_component_dir`] but creates one child directory per name, each
/// holding a `cbs.component.yaml` placeholder, so that
/// `cbsd-server::components::tarball::pack_components(tempdir.path(), names)`
/// succeeds for a multi-component build. The returned `TempDir` cleans up on
/// drop; callers must keep it alive for the duration of the test.
pub fn temp_components_dir(component_names: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    for name in component_names {
        let component_root = tmp.path().join(name);
        std::fs::create_dir_all(&component_root).expect("mkdir component");
        std::fs::write(
            component_root.join("cbs.component.yaml"),
            format!("name: {name}\n").into_bytes(),
        )
        .expect("write component yaml");
    }
    tmp
}

/// Build a `SessionManagerLayer<SqliteStore, SignedCookie>` for tests
/// that exercise the full `build_router` chain. Runs the tower-sessions
/// SQLite migration and generates a fresh signing key per call.
pub async fn test_session_layer(
    pool: SqlitePool,
) -> SessionManagerLayer<SqliteStore, SignedCookie> {
    let session_store = SqliteStore::new(pool);
    session_store
        .migrate()
        .await
        .expect("session store migrate");
    let session_key = tower_sessions::cookie::Key::generate();
    SessionManagerLayer::new(session_store)
        .with_signed(session_key)
        .with_name("cbsd_test_session")
        .with_http_only(true)
}

/// Fixed `retry_at` epoch used by [`seed_periodic_task_in_retry`], so
/// tests can assert the seeded retry state survives untouched.
pub const TEST_RETRY_AT: i64 = 4_102_444_800;

/// Seed `owner` (real user row) plus a periodic task in mid-retry
/// state: disabled, `retry_count = 3`, `retry_at = TEST_RETRY_AT`,
/// `last_error = "boom"`, stored priority `low`. Manual-trigger tests
/// (design 024) assert this exact state is preserved by a trigger.
pub async fn seed_periodic_task_in_retry(
    pool: &SqlitePool,
    task_id: &str,
    owner: &str,
    descriptor: &str,
    tag_format: &str,
) {
    crate::db::users::create_or_update_user(pool, owner, "Owner")
        .await
        .expect("seed owner");
    crate::db::periodic::insert_task(
        pool,
        task_id,
        "0 2 * * *",
        tag_format,
        descriptor,
        "low",
        None,
        owner,
    )
    .await
    .expect("insert task");
    sqlx::query!(
        r#"UPDATE periodic_tasks
           SET enabled = 0, retry_count = 3, retry_at = ?, last_error = 'boom'
           WHERE id = ?"#,
        TEST_RETRY_AT,
        task_id,
    )
    .execute(pool)
    .await
    .expect("seed retry state");
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

/// Seed a fully authenticated user for tests that drive the real router
/// (`build_router` + `oneshot`): user row, a dedicated role carrying
/// `caps` with no scopes (scope-less assignments pass every scope
/// check), and a valid PASETO token whose hash is registered in the
/// `tokens` table. Returns the raw bearer token for an
/// `Authorization: Bearer <token>` header. The token is minted with
/// [`test_token_secret`], the same key every test `ServerConfig` uses.
pub async fn seed_authed_bearer(pool: &SqlitePool, email: &str, caps: &[&str]) -> String {
    use secrecy::ExposeSecret;

    crate::db::users::create_or_update_user(pool, email, "Test User")
        .await
        .expect("seed user");
    let role_name = format!("test-role-{}", email.replace(['@', '.'], "-"));
    crate::db::roles::create_role(pool, &role_name, "test role", false)
        .await
        .expect("create role");
    crate::db::roles::set_role_caps_and_scopes(pool, &role_name, caps, &[])
        .await
        .expect("set role caps");
    crate::db::roles::add_user_role(pool, email, &role_name)
        .await
        .expect("assign role");

    let (token, hash) =
        crate::auth::paseto::token_create(email, 3600, &test_token_secret()).expect("mint token");
    crate::db::tokens::insert_token(pool, email, &hash, None)
        .await
        .expect("insert token");
    token.expose_secret().to_string()
}
