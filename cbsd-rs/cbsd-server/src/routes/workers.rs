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

//! Route handlers for `/api/workers`: worker listing.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::app::AppState;
use crate::auth::extractors::{auth_error, AuthUser, ErrorDetail};
use crate::queue::WorkerInfo;

/// Build the workers sub-router: `/api/workers`.
pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list_workers))
}

/// `GET /api/workers` — list connected workers. Requires `workers:view`.
async fn list_workers(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<WorkerInfo>>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("workers:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: workers:view",
        ));
    }

    let workers = {
        let queue = state.queue.lock().await;
        queue.connected_workers()
    };

    Ok(Json(workers))
}
