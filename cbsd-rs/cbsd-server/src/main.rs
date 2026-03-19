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

mod app;
mod auth;
mod components;
mod config;
mod db;
mod logs;
mod queue;
mod routes;
mod scheduler;
mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use hkdf::Hkdf;
use sha2::Sha256;
use tower_sessions::session_store::ExpiredDeletion;
use tower_sessions_sqlx_store::SqliteStore;
use tracing_subscriber::EnvFilter;

/// CBS build service daemon (Rust).
#[derive(Parser)]
#[command(name = "cbsd-server", about = "CBS build service daemon")]
struct Cli {
    /// Path to server config YAML file.
    #[arg(short, long)]
    config: PathBuf,

    /// Drain mode: revoke all active builds before shutdown.
    #[arg(long)]
    drain: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Load and validate config
    let config = config::load_config(&cli.config);

    // Set up tracing
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("cbsd-server starting");

    // Create SQLite pool with pragmas
    let db_url = format!("sqlite://{}", config.db_path);
    let pool = db::create_pool(&db_url).await;

    // Run sqlx migrations
    tracing::info!("running database migrations");
    db::run_migrations(&pool).await;

    // Initialize tower-sessions SQLite store (creates tower_sessions table)
    let session_store = SqliteStore::new(pool.clone());
    session_store
        .migrate()
        .await
        .expect("failed to initialize session store");

    // First-startup seed: create builtin roles, admin user, worker API keys
    // (only runs if roles table is empty).
    db::seed::run_first_startup_seed(&pool, &config)
        .await
        .expect("first-startup seed failed");

    // Derive session signing key from token_secret_key via HKDF-SHA256
    // (deterministic across restarts, domain-separated from PASETO key)
    let token_key_bytes = config.secrets.token_secret_key.as_bytes();
    let hk = Hkdf::<Sha256>::new(None, token_key_bytes);
    let mut session_key_bytes = [0u8; 64];
    hk.expand(b"cbsd-oauth-session-v1", &mut session_key_bytes)
        .expect("HKDF expand failed for session key");
    let session_key = tower_sessions::cookie::Key::from(&session_key_bytes);

    // Session layer with signed cookies and 10-minute expiry (OAuth flows only)
    let session_layer = tower_sessions::SessionManagerLayer::new(session_store.clone())
        .with_signed(session_key)
        .with_expiry(tower_sessions::Expiry::OnInactivity(
            time::Duration::minutes(10),
        ));

    // Spawn background task to delete expired sessions
    let _deletion_task = tokio::task::spawn(
        session_store
            .clone()
            .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
    );

    // Load Google OAuth configuration from secrets file
    let oauth = auth::oauth::load_oauth_config(&config.oauth.secrets_file)
        .expect("failed to load OAuth secrets");
    tracing::info!("loaded OAuth configuration");

    // Create API key LRU cache (capacity: 512)
    let api_key_cache = auth::api_keys::ApiKeyCache::new(512);

    // Create the in-memory build queue
    let build_queue = Arc::new(tokio::sync::Mutex::new(queue::BuildQueue::new()));

    // Load component definitions
    let loaded_components = if config.components_dir.exists() {
        components::load_components(&config.components_dir).unwrap_or_else(|e| {
            tracing::warn!(
                "failed to load components from {}: {e}",
                config.components_dir.display()
            );
            Vec::new()
        })
    } else {
        tracing::warn!(
            "components directory {} does not exist — no components loaded",
            config.components_dir.display()
        );
        Vec::new()
    };
    tracing::info!("loaded {} component(s)", loaded_components.len());

    // Build app state and router
    let worker_senders = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let log_watchers = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let log_writer = Arc::new(tokio::sync::Mutex::new(logs::writer::LogWriterState::new()));

    // Startup recovery: reconcile in-flight builds from prior instance.
    // Must run after migrations and before accepting connections.
    queue::recovery::run_startup_recovery(&pool, &build_queue, &log_watchers)
        .await
        .expect("startup recovery failed — aborting");

    let sweep_handle = Arc::new(tokio::sync::Mutex::new(None));

    let gc_handle = Arc::new(tokio::sync::Mutex::new(None));

    let scheduler_notify = Arc::new(tokio::sync::Notify::new());
    let scheduler_handle = Arc::new(tokio::sync::Mutex::new(None));

    let state = app::AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        oauth,
        api_key_cache,
        queue: build_queue,
        components: loaded_components,
        worker_senders,
        log_watchers,
        log_writer,
        sweep_handle: sweep_handle.clone(),
        gc_handle: gc_handle.clone(),
        scheduler_notify: scheduler_notify.clone(),
        scheduler_handle: scheduler_handle.clone(),
    };

    // Start the periodic re-dispatch sweep.
    let handle = ws::dispatch::start_periodic_sweep(&state);
    {
        let mut guard = sweep_handle.lock().await;
        *guard = Some(handle);
    }
    tracing::info!("periodic dispatch sweep started (30s interval)");

    // Start the periodic log GC task (first tick delayed 24h).
    let gc_task_handle = logs::gc::start_log_gc(
        state.pool.clone(),
        state.config.log_dir.clone(),
        state.config.log_retention.log_retention_days,
    );
    {
        let mut guard = gc_handle.lock().await;
        *guard = Some(gc_task_handle);
    }
    tracing::info!(
        retention_days = state.config.log_retention.log_retention_days,
        "log GC task started (24h interval, first tick delayed)"
    );

    // Start the periodic build scheduler.
    let sched_task_handle = tokio::spawn(scheduler::run_scheduler(state.clone(), scheduler_notify));
    {
        let mut guard = scheduler_handle.lock().await;
        *guard = Some(sched_task_handle);
    }
    tracing::info!("periodic build scheduler started");

    let router = app::build_router(state.clone(), session_layer);

    // Start server
    let addr: SocketAddr = state
        .config
        .listen_addr
        .parse()
        .expect("invalid listen_addr");
    tracing::info!("listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    // Use a shared shutdown mode so we can read it after the server stops.
    let shutdown_mode = Arc::new(tokio::sync::Mutex::new(ShutdownMode::Restart));
    let shutdown_mode_clone = shutdown_mode.clone();

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(cli.drain, shutdown_mode_clone))
    .await
    .expect("server error");

    // Post-shutdown cleanup based on mode.
    let mode = *shutdown_mode.lock().await;
    tracing::info!(mode = ?mode, "server stopped accepting connections");

    if mode == ShutdownMode::Drain {
        run_drain_shutdown(&state).await;
    }

    // Abort background tasks.
    {
        let guard = state.sweep_handle.lock().await;
        if let Some(h) = guard.as_ref() {
            h.abort();
        }
    }
    {
        let guard = state.gc_handle.lock().await;
        if let Some(h) = guard.as_ref() {
            h.abort();
        }
    }
    {
        let guard = state.scheduler_handle.lock().await;
        if let Some(h) = guard.as_ref() {
            h.abort();
        }
    }

    tracing::info!("cbsd-server shut down");
}

/// Shutdown mode determined by the signal received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownMode {
    /// SIGTERM / Ctrl+C: stop accepting connections, let workers reconnect
    /// to the new instance. Do not revoke active builds.
    Restart,
    /// SIGQUIT / --drain: revoke active builds, wait for acks, then shut down.
    Drain,
}

async fn shutdown_signal(drain: bool, mode: Arc<tokio::sync::Mutex<ShutdownMode>>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    #[cfg(unix)]
    let quit = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::quit())
            .expect("failed to install SIGQUIT handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let quit = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("received Ctrl+C — graceful restart");
            *mode.lock().await = ShutdownMode::Restart;
        }
        () = terminate => {
            tracing::info!("received SIGTERM — graceful restart (no revoke)");
            *mode.lock().await = ShutdownMode::Restart;
        }
        () = quit => {
            if drain {
                tracing::info!("received SIGQUIT — drain mode (will revoke active builds)");
                *mode.lock().await = ShutdownMode::Drain;
            } else {
                tracing::info!("received SIGQUIT — decommission mode (will revoke active builds)");
                *mode.lock().await = ShutdownMode::Drain;
            }
        }
    }
}

/// Drain shutdown: revoke active builds, wait for acks, mark stragglers
/// as failures.
async fn run_drain_shutdown(state: &app::AppState) {
    let drain_timeout =
        tokio::time::Duration::from_secs(state.config.timeouts.revoke_ack_timeout_secs);

    // Collect active build IDs and their worker connection IDs.
    let active_builds: Vec<(i64, String)> = {
        let queue = state.queue.lock().await;
        queue
            .active
            .iter()
            .map(|(build_id, ab)| (*build_id, ab.connection_id.clone()))
            .collect()
    };

    if active_builds.is_empty() {
        tracing::info!("drain: no active builds to revoke");
        return;
    }

    tracing::info!(count = active_builds.len(), "drain: revoking active builds");

    // Send build_revoke to each worker and mark builds as revoking.
    for (build_id, connection_id) in &active_builds {
        // Mark build as revoking in DB.
        if let Err(e) = db::builds::set_build_revoking(&state.pool, *build_id).await {
            tracing::error!(
                build_id = build_id,
                "drain: failed to set revoking state: {e}"
            );
        }

        // Send revoke message to worker.
        let msg = cbsd_proto::ws::ServerMessage::BuildRevoke {
            build_id: cbsd_proto::BuildId(*build_id),
        };
        if let Ok(json) = serde_json::to_string(&msg) {
            let senders = state.worker_senders.lock().await;
            if let Some(tx) = senders.get(connection_id.as_str()) {
                let _ = tx.send(axum::extract::ws::Message::Text(json.into()));
            }
        }
    }

    // Wait for the drain timeout, allowing workers to send build_finished acks.
    tracing::info!(
        timeout_secs = drain_timeout.as_secs(),
        "drain: waiting for build_finished acks"
    );
    tokio::time::sleep(drain_timeout).await;

    // Mark any still-active builds as failures.
    let remaining: Vec<i64> = {
        let queue = state.queue.lock().await;
        queue.active.keys().copied().collect()
    };

    for build_id in &remaining {
        tracing::warn!(
            build_id = build_id,
            "drain: build did not finish in time — marking as failure"
        );
        if let Err(e) = db::builds::set_build_finished(
            &state.pool,
            *build_id,
            "failure",
            Some("server decommissioned"),
        )
        .await
        {
            tracing::error!(
                build_id = build_id,
                "drain: failed to mark build as failure: {e}"
            );
        }
        // Finalize log.
        if let Err(e) = db::builds::set_build_log_finished(&state.pool, *build_id).await {
            tracing::error!(
                build_id = build_id,
                "drain: failed to finalize build log: {e}"
            );
        }
    }

    if remaining.is_empty() {
        tracing::info!("drain: all builds acknowledged revocation");
    } else {
        tracing::warn!(
            count = remaining.len(),
            "drain: marked unacknowledged builds as failure"
        );
    }
}
