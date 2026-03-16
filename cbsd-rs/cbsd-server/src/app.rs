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

use std::collections::HashMap;
use std::sync::Arc;

use crate::ws;
use axum::extract::ws::Message;
use axum::{Json, Router, routing::get};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, mpsc, watch};
use tower_sessions::SessionManagerLayer;
use tower_sessions::service::SignedCookie;
use tower_sessions_sqlx_store::SqliteStore;

use crate::auth::api_keys::ApiKeyCache;
use crate::auth::oauth::OAuthState;
use crate::components::ComponentInfo;
use crate::config::ServerConfig;
use crate::logs::writer::SharedLogWriter;
use crate::queue::SharedBuildQueue;
use crate::routes;

/// Per-worker channel sender for outbound WebSocket messages.
/// The WS handler loop reads from the receiver and forwards to the socket.
pub type WorkerSender = mpsc::UnboundedSender<Message>;

/// Map of connection_id -> WorkerSender for all connected workers.
pub type WorkerSenders = Arc<Mutex<HashMap<String, WorkerSender>>>;

/// Map of build_id -> watch::Sender for log file change notifications.
pub type LogWatchers = Arc<Mutex<HashMap<i64, watch::Sender<()>>>>;

/// Shared application state. Extended by subsequent commits.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<ServerConfig>,
    pub oauth: OAuthState,
    pub api_key_cache: Arc<Mutex<ApiKeyCache>>,
    pub queue: SharedBuildQueue,
    pub components: Vec<ComponentInfo>,
    /// Per-worker outbound message channels.
    pub worker_senders: WorkerSenders,
    /// Build log file change watchers (notifies SSE/follow endpoints).
    pub log_watchers: LogWatchers,
    /// Build log writer state (seq-to-offset indices).
    pub log_writer: SharedLogWriter,
    /// Handle for the periodic re-dispatch sweep task.
    pub sweep_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Handle for the log GC background task.
    pub gc_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

/// Build the axum router.
pub fn build_router(
    state: AppState,
    session_layer: SessionManagerLayer<SqliteStore, SignedCookie>,
) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .nest("/auth", routes::auth::router())
        .nest("/permissions", routes::permissions::router())
        .nest("/admin", routes::admin::router())
        .nest("/builds", routes::builds::router())
        .nest("/components", routes::components::router())
        .nest("/workers", routes::workers::router());

    Router::new()
        .nest("/api", api.route("/ws/worker", get(ws::handler::ws_upgrade)))
        .layer(session_layer)
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}
