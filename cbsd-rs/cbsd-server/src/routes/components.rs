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

//! Route handlers for `/api/components/*`: component discovery.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail};
use crate::components::ComponentInfo;

/// Build the components sub-router: `/api/components/*`.
pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list_components))
}

// ---------------------------------------------------------------------------
// GET /api/components/
// ---------------------------------------------------------------------------

/// List all known components. Requires authentication but no specific
/// capability.
async fn list_components(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Vec<ComponentInfo>>, (StatusCode, Json<ErrorDetail>)> {
    Ok(Json(state.components.clone()))
}
