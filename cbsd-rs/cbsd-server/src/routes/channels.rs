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

//! Route handlers for `/api/channels/*`: channel and type CRUD.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error};
use crate::db;
use crate::scopes;

/// Build the channels sub-router: `/api/channels/*`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_channel))
        .route("/", get(list_channels))
        .route("/{id}", get(get_channel))
        .route("/{id}", put(update_channel))
        .route("/{id}", delete(delete_channel))
        .route("/{id}/types", post(add_type))
        .route("/{id}/types/{tid}", put(update_type))
        .route("/{id}/types/{tid}", delete(delete_type))
        .route("/{id}/default-type", put(set_default_type))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateChannelBody {
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct UpdateChannelBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct AddTypeBody {
    type_name: String,
    project: String,
    #[serde(default)]
    prefix_template: String,
}

#[derive(Deserialize)]
struct UpdateTypeBody {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    prefix_template: Option<String>,
}

#[derive(Deserialize)]
struct SetDefaultTypeBody {
    type_id: i64,
}

#[derive(Serialize)]
struct ChannelResponse {
    id: i64,
    name: String,
    description: String,
    default_type_id: Option<i64>,
    types: Vec<TypeResponse>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Serialize)]
struct TypeResponse {
    id: i64,
    type_name: String,
    project: String,
    prefix_template: String,
    created_at: i64,
    updated_at: i64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// NOTE: also defined in routes::permissions — kept local to avoid a cross-module
// public export for a small helper.
fn require_cap(user: &AuthUser, cap: &str) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap(cap) {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            &format!("missing required capability: {cap}"),
        ));
    }
    Ok(())
}

/// Build a full channel response with inline types.
async fn build_channel_response(
    pool: &sqlx::SqlitePool,
    channel: db::channels::ChannelRecord,
) -> Result<ChannelResponse, (StatusCode, Json<ErrorDetail>)> {
    let types = db::channels::list_types_for_channel(pool, channel.id)
        .await
        .map_err(|e| {
            tracing::error!("failed to list types for channel {}: {e}", channel.id);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list channel types",
            )
        })?;

    Ok(ChannelResponse {
        id: channel.id,
        name: channel.name,
        description: channel.description,
        default_type_id: channel.default_type_id,
        types: types.into_iter().map(type_to_response).collect(),
        created_at: channel.created_at,
        updated_at: channel.updated_at,
    })
}

fn type_to_response(t: db::channels::ChannelTypeRecord) -> TypeResponse {
    TypeResponse {
        id: t.id,
        type_name: t.type_name,
        project: t.project,
        prefix_template: t.prefix_template,
        created_at: t.created_at,
        updated_at: t.updated_at,
    }
}

/// Check whether the user has any channel scope that grants access to view
/// a specific channel. Returns true if the user has `*` cap, or if any of
/// their channel scopes match `channel_name/*` or a specific type.
async fn user_can_view_channel(
    pool: &sqlx::SqlitePool,
    user: &AuthUser,
    channel_name: &str,
) -> Result<bool, (StatusCode, Json<ErrorDetail>)> {
    // Admin wildcard sees everything.
    if user.has_cap("*") || user.has_cap("channels:manage") || user.has_cap("channels:view") {
        return Ok(true);
    }

    // Check channel scopes: any scope pattern that starts with `channel_name/`
    // or is `*` grants visibility.
    let assignments = db::roles::get_user_assignments_with_scopes(pool, &user.email)
        .await
        .map_err(|e| {
            tracing::warn!("failed to load scope assignments for '{}': {e}", user.email);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check channel visibility",
            )
        })?;

    Ok(assignments.iter().any(|a| {
        if a.scopes.is_empty() {
            return true;
        }
        a.scopes.iter().any(|s| {
            s.scope_type == "channel" && scopes::scope_covers_channel(&s.pattern, channel_name)
        })
    }))
}

// ---------------------------------------------------------------------------
// POST /api/channels
// ---------------------------------------------------------------------------

async fn create_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateChannelBody>,
) -> Result<(StatusCode, Json<ChannelResponse>), (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    if body.name.is_empty() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "channel name must not be empty",
        ));
    }

    let id = db::channels::create_channel(&state.pool, &body.name, &body.description)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                return auth_error(
                    StatusCode::CONFLICT,
                    &format!("channel '{}' already exists", body.name),
                );
            }
            tracing::error!("failed to create channel '{}': {e}", body.name);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create channel",
            )
        })?;

    let channel = db::channels::get_channel_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "channel not found after create",
            )
        })?;

    let resp = build_channel_response(&state.pool, channel).await?;

    tracing::info!(
        "user {} created channel '{}' (id={})",
        user.email,
        body.name,
        id
    );

    Ok((StatusCode::CREATED, Json(resp)))
}

// ---------------------------------------------------------------------------
// GET /api/channels
// ---------------------------------------------------------------------------

async fn list_channels(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ChannelResponse>>, (StatusCode, Json<ErrorDetail>)> {
    let channels = db::channels::list_active_channels(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("failed to list channels: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list channels")
        })?;

    let mut result = Vec::with_capacity(channels.len());
    for channel in channels {
        // Filter by visibility: user must have some scope for this channel.
        if user_can_view_channel(&state.pool, &user, &channel.name).await? {
            let resp = build_channel_response(&state.pool, channel).await?;
            result.push(resp);
        }
    }

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// GET /api/channels/{id}
// ---------------------------------------------------------------------------

async fn get_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<ChannelResponse>, (StatusCode, Json<ErrorDetail>)> {
    let channel = db::channels::get_channel_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    if !user_can_view_channel(&state.pool, &user, &channel.name).await? {
        return Err(auth_error(StatusCode::NOT_FOUND, "channel not found"));
    }

    let resp = build_channel_response(&state.pool, channel).await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// PUT /api/channels/{id}
// ---------------------------------------------------------------------------

async fn update_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateChannelBody>,
) -> Result<Json<ChannelResponse>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    let existing = db::channels::get_channel_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    let name = body.name.unwrap_or(existing.name);
    let description = body.description.unwrap_or(existing.description);

    if name.is_empty() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "channel name must not be empty",
        ));
    }

    db::channels::update_channel(&state.pool, id, &name, &description)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                return auth_error(
                    StatusCode::CONFLICT,
                    &format!("channel name '{name}' already in use"),
                );
            }
            tracing::error!("failed to update channel {id}: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update channel",
            )
        })?;

    let channel = db::channels::get_channel_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found after update"))?;

    let resp = build_channel_response(&state.pool, channel).await?;

    tracing::info!("user {} updated channel {id}", user.email);

    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// DELETE /api/channels/{id}
// ---------------------------------------------------------------------------

async fn delete_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    let deleted = db::channels::soft_delete_channel(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to soft-delete channel {id}: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete channel",
            )
        })?;

    if !deleted {
        return Err(auth_error(StatusCode::NOT_FOUND, "channel not found"));
    }

    tracing::info!("user {} soft-deleted channel {id}", user.email);

    Ok(Json(
        serde_json::json!({"detail": format!("channel {id} deleted")}),
    ))
}

// ---------------------------------------------------------------------------
// POST /api/channels/{id}/types
// ---------------------------------------------------------------------------

async fn add_type(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<AddTypeBody>,
) -> Result<(StatusCode, Json<TypeResponse>), (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    // Validate type_name.
    let valid_types = ["dev", "release", "test", "ci"];
    if !valid_types.contains(&body.type_name.as_str()) {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid type_name '{}': must be one of dev, release, test, ci",
                body.type_name
            ),
        ));
    }

    // Verify channel exists.
    let channel = db::channels::get_channel_by_id(&state.pool, channel_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {channel_id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    let type_id = db::channels::create_type(
        &state.pool,
        channel_id,
        &body.type_name,
        &body.project,
        &body.prefix_template,
    )
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            return auth_error(
                StatusCode::CONFLICT,
                &format!(
                    "type '{}' already exists for channel '{}'",
                    body.type_name, channel.name
                ),
            );
        }
        tracing::error!("failed to create type for channel {channel_id}: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create type")
    })?;

    // Auto-set default_type_id if this is the first type for the channel.
    if channel.default_type_id.is_none()
        && let Err(e) = db::channels::set_default_type(&state.pool, channel_id, type_id).await
    {
        tracing::warn!("failed to auto-set default type for channel {channel_id}: {e}");
    }

    let ct = db::channels::get_type(&state.pool, type_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get type {type_id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "type not found after create",
            )
        })?;

    tracing::info!(
        "user {} added type '{}' to channel '{}' (type_id={type_id})",
        user.email,
        body.type_name,
        channel.name,
    );

    Ok((StatusCode::CREATED, Json(type_to_response(ct))))
}

// ---------------------------------------------------------------------------
// PUT /api/channels/{id}/types/{tid}
// ---------------------------------------------------------------------------

async fn update_type(
    State(state): State<AppState>,
    user: AuthUser,
    Path((channel_id, tid)): Path<(i64, i64)>,
    Json(body): Json<UpdateTypeBody>,
) -> Result<Json<TypeResponse>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    // Verify channel exists.
    db::channels::get_channel_by_id(&state.pool, channel_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {channel_id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    // Get existing type.
    let existing = db::channels::get_type(&state.pool, tid)
        .await
        .map_err(|e| {
            tracing::error!("failed to get type {tid}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "type not found"))?;

    if existing.channel_id != channel_id {
        return Err(auth_error(
            StatusCode::NOT_FOUND,
            "type does not belong to this channel",
        ));
    }

    let project = body.project.unwrap_or(existing.project);
    let prefix_template = body.prefix_template.unwrap_or(existing.prefix_template);

    db::channels::update_type(&state.pool, tid, &project, &prefix_template)
        .await
        .map_err(|e| {
            tracing::error!("failed to update type {tid}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to update type")
        })?;

    let ct = db::channels::get_type(&state.pool, tid)
        .await
        .map_err(|e| {
            tracing::error!("failed to get type {tid}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "type not found after update"))?;

    tracing::info!("user {} updated type {tid}", user.email);

    Ok(Json(type_to_response(ct)))
}

// ---------------------------------------------------------------------------
// DELETE /api/channels/{id}/types/{tid}
// ---------------------------------------------------------------------------

async fn delete_type(
    State(state): State<AppState>,
    user: AuthUser,
    Path((channel_id, tid)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    // Verify channel exists.
    db::channels::get_channel_by_id(&state.pool, channel_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {channel_id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    // Verify type belongs to channel.
    let existing = db::channels::get_type(&state.pool, tid)
        .await
        .map_err(|e| {
            tracing::error!("failed to get type {tid}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "type not found"))?;

    if existing.channel_id != channel_id {
        return Err(auth_error(
            StatusCode::NOT_FOUND,
            "type does not belong to this channel",
        ));
    }

    let deleted = db::channels::soft_delete_type(&state.pool, tid)
        .await
        .map_err(|e| {
            tracing::error!("failed to soft-delete type {tid}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete type")
        })?;

    if !deleted {
        return Err(auth_error(StatusCode::NOT_FOUND, "type not found"));
    }

    tracing::info!("user {} soft-deleted type {tid}", user.email);

    Ok(Json(
        serde_json::json!({"detail": format!("type {tid} deleted")}),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/channels/{id}/default-type
// ---------------------------------------------------------------------------

async fn set_default_type(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<SetDefaultTypeBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "channels:manage")?;

    // Verify channel exists.
    db::channels::get_channel_by_id(&state.pool, channel_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get channel {channel_id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    // Verify type exists and belongs to channel.
    let ct = db::channels::get_type(&state.pool, body.type_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get type {}: {e}", body.type_id);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "type not found"))?;

    if ct.channel_id != channel_id {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "type does not belong to this channel",
        ));
    }

    db::channels::set_default_type(&state.pool, channel_id, body.type_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to set default type for channel {channel_id}: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set default type",
            )
        })?;

    tracing::info!(
        "user {} set default type for channel {channel_id} to {}",
        user.email,
        body.type_id,
    );

    Ok(Json(serde_json::json!({
        "detail": format!("default type set to {}", body.type_id),
    })))
}

// NOTE: duplicates `db::is_unique_violation` but pre-dates the shared
// helper and lives outside the robot-accounts phase's scope. Safe to
// migrate to `db::is_unique_violation` in a future opportunistic cleanup.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        db_err.code().is_some_and(|c| c == "2067")
    } else {
        false
    }
}
