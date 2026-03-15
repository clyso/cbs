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
mod config;
mod db;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
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
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.logging.level));
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

    // Session layer with 10-minute expiry (OAuth flows only)
    let session_layer = tower_sessions::SessionManagerLayer::new(session_store.clone())
        .with_expiry(tower_sessions::Expiry::OnInactivity(
            time::Duration::minutes(10),
        ));

    // Spawn background task to delete expired sessions
    let _deletion_task = tokio::task::spawn(
        session_store
            .clone()
            .continuously_delete_expired(tokio::time::Duration::from_secs(60)),
    );

    // Build app state and router
    let state = app::AppState {
        pool: pool.clone(),
        config: Arc::new(config),
    };
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

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(cli.drain))
        .await
        .expect("server error");

    tracing::info!("cbsd-server shut down");
}

async fn shutdown_signal(drain: bool) {
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
            tracing::info!("received Ctrl+C, shutting down");
        }
        () = terminate => {
            tracing::info!("received SIGTERM — graceful restart (no revoke)");
        }
        () = quit => {
            if drain {
                tracing::info!("received SIGQUIT — drain mode");
            } else {
                tracing::info!("received SIGQUIT — decommission mode");
            }
        }
    }
}
