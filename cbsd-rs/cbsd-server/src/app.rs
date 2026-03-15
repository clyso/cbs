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

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use sqlx::SqlitePool;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::SqliteStore;

use crate::config::ServerConfig;

/// Shared application state. Extended by subsequent commits.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<ServerConfig>,
}

/// Build the axum router.
pub fn build_router(state: AppState, session_layer: SessionManagerLayer<SqliteStore>) -> Router {
    let api = Router::new().route("/health", get(health));

    Router::new()
        .nest("/api", api)
        .layer(session_layer)
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}
