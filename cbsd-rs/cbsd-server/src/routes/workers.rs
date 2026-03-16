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

//! Route handlers for `/api/workers`: registered worker listing.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use cbsd_proto::Arch;

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error};
use crate::db;
use crate::ws::liveness::WorkerState;

/// Build the workers sub-router: `/api/workers`.
pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list_workers))
}

/// Response item for the merged worker listing.
#[derive(Debug, Serialize)]
struct WorkerInfoResponse {
    worker_id: String,
    name: String,
    arch: Arch,
    status: String,
    last_seen: Option<i64>,
    created_by: String,
    created_at: i64,
    current_build_id: Option<i64>,
}

/// `GET /api/workers` — list all registered workers with live status.
///
/// Merges the `workers` DB table (all registered) with the in-memory
/// `BuildQueue.workers` map to produce a unified view including offline
/// workers.
async fn list_workers(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<WorkerInfoResponse>>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("workers:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: workers:view",
        ));
    }

    // 1. Query all registered workers from DB.
    let db_workers = db::workers::list_workers(&state.pool).await.map_err(|e| {
        tracing::error!("failed to list workers: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // 2. Snapshot in-memory status: registered_worker_id -> (status, build_id).
    let status_map: HashMap<String, (String, Option<i64>)> = {
        let queue = state.queue.lock().await;
        let mut map = HashMap::new();

        for (cid, ws) in &queue.workers {
            let Some(reg_id) = ws.registered_worker_id() else {
                continue;
            };

            let active_build_id = queue
                .active
                .values()
                .find(|ab| ab.connection_id == *cid)
                .map(|ab| ab.build_id);

            let status = match ws {
                WorkerState::Connected { .. } => {
                    if active_build_id.is_some() {
                        "building"
                    } else {
                        "connected"
                    }
                }
                WorkerState::Stopping { .. } => "stopping",
                WorkerState::Disconnected { .. } => "disconnected",
                WorkerState::Dead => "dead",
            };

            map.insert(reg_id.to_string(), (status.to_string(), active_build_id));
        }

        map
    };

    // 3. Merge DB rows with in-memory status.
    let result: Vec<WorkerInfoResponse> = db_workers
        .into_iter()
        .map(|row| {
            let (status, current_build_id) = status_map
                .get(&row.id)
                .cloned()
                .unwrap_or_else(|| ("offline".to_string(), None));

            let arch = match row.arch.as_str() {
                "x86_64" => Arch::X86_64,
                "aarch64" => Arch::Aarch64,
                other => {
                    tracing::warn!(
                        worker_id = %row.id,
                        arch = %other,
                        "invalid arch in DB — defaulting to x86_64"
                    );
                    Arch::X86_64
                }
            };

            WorkerInfoResponse {
                worker_id: row.id,
                name: row.name,
                arch,
                status,
                last_seen: row.last_seen,
                created_by: row.created_by,
                created_at: row.created_at,
                current_build_id,
            }
        })
        .collect();

    Ok(Json(result))
}
