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
use std::time::Duration;

use crate::ws;
use axum::extract::ws::Message;
use axum::http::{HeaderName, Request};
use axum::{Json, Router, routing::get};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, mpsc, watch};
use tower_http::classify::ServerErrorsFailureClass;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::TraceLayer;
use tower_sessions::SessionManagerLayer;
use tower_sessions::service::SignedCookie;
use tower_sessions_sqlx_store::SqliteStore;
use tracing::Span;

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
    /// Notify handle for the periodic build scheduler (wakes on task changes).
    pub scheduler_notify: Arc<tokio::sync::Notify>,
    /// Handle for the periodic build scheduler task.
    pub scheduler_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

/// Header name used for request IDs (propagated to responses).
static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

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
        .nest("/workers", routes::workers::router())
        .nest("/periodic", routes::periodic::router());

    // Request/response tracing: logs method, URI, status, and latency
    // for every HTTP request. The request ID is generated per-request
    // and included in both the tracing span and the response header.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &Request<_>| {
            let request_id = request
                .headers()
                .get(&X_REQUEST_ID)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-");
            tracing::info_span!(
                "request",
                method = %request.method(),
                uri = %request.uri(),
                request_id = %request_id,
            )
        })
        .on_response(
            |response: &axum::http::Response<_>,
             latency: Duration,
             _span: &Span| {
                let status = response.status().as_u16();
                let latency_ms = latency.as_millis();
                if status >= 500 {
                    tracing::error!(
                        status, latency_ms, "response"
                    );
                } else if status >= 400 {
                    tracing::warn!(
                        status, latency_ms, "response"
                    );
                } else {
                    tracing::info!(
                        status, latency_ms, "response"
                    );
                }
            },
        )
        .on_failure(
            |error: ServerErrorsFailureClass,
             latency: Duration,
             _span: &Span| {
                tracing::error!(
                    latency_ms = latency.as_millis(),
                    "request failed: {error}"
                );
            },
        );

    // Redact Authorization header from debug-level logs to avoid
    // leaking tokens.
    let sensitive_headers_layer = SetSensitiveRequestHeadersLayer::new([
        axum::http::header::AUTHORIZATION,
    ]);

    // Layer ordering (outermost → innermost):
    //   1. SetRequestId — assigns x-request-id before tracing sees it
    //   2. Sensitive headers — marks Authorization as sensitive
    //   3. TraceLayer — logs request/response with the assigned ID
    //   4. PropagateRequestId — copies x-request-id to the response
    //   5. SessionManagerLayer — session handling for OAuth
    Router::new()
        .nest(
            "/api",
            api.route("/ws/worker", get(ws::handler::ws_upgrade)),
        )
        .layer(session_layer)
        .layer(PropagateRequestIdLayer::new(X_REQUEST_ID.clone()))
        .layer(trace_layer)
        .layer(sensitive_headers_layer)
        .layer(SetRequestIdLayer::new(
            X_REQUEST_ID.clone(),
            MakeRequestUuid,
        ))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}
