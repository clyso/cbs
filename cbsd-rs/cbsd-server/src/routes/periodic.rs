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

//! Route handlers for `/api/periodic/*`: periodic build task management.

use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, ScopeType, auth_error};
use crate::db;
use crate::db::periodic::PeriodicTaskRow;
use crate::scheduler::tag_format;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct PeriodicTaskResponse {
    id: String,
    cron_expr: String,
    tag_format: String,
    descriptor: serde_json::Value,
    priority: String,
    summary: Option<String>,
    enabled: bool,
    created_by: String,
    created_at: i64,
    updated_at: i64,
    retry_count: i64,
    retry_at: Option<i64>,
    last_error: Option<String>,
    last_triggered_at: Option<i64>,
    last_build_id: Option<i64>,
    next_run: Option<i64>,
}

/// Convert a database row to an API response, computing `next_run` from the
/// cron expression (or `retry_at` if the task is retrying).
fn task_to_response(row: PeriodicTaskRow) -> PeriodicTaskResponse {
    let next_run = if !row.enabled {
        None
    } else if let Some(retry_at) = row.retry_at {
        Some(retry_at)
    } else {
        croner::Cron::from_str(&row.cron_expr)
            .ok()
            .and_then(|cron| {
                let now = chrono::Utc::now();
                cron.find_next_occurrence(&now, false).ok()
            })
            .map(|dt| dt.timestamp())
    };

    let descriptor = serde_json::from_str(&row.descriptor)
        .unwrap_or_else(|_| serde_json::Value::String(row.descriptor.clone()));

    PeriodicTaskResponse {
        id: row.id,
        cron_expr: row.cron_expr,
        tag_format: row.tag_format,
        descriptor,
        priority: row.priority,
        summary: row.summary,
        enabled: row.enabled,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
        retry_count: row.retry_count,
        retry_at: row.retry_at,
        last_error: row.last_error,
        last_triggered_at: row.last_triggered_at,
        last_build_id: row.last_build_id,
        next_run,
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateTaskBody {
    cron_expr: String,
    tag_format: String,
    descriptor: serde_json::Value,
    #[serde(default = "default_priority")]
    priority: String,
    summary: Option<String>,
}

fn default_priority() -> String {
    "normal".to_string()
}

#[derive(Deserialize)]
struct UpdateTaskBody {
    cron_expr: Option<String>,
    tag_format: Option<String>,
    descriptor: Option<serde_json::Value>,
    priority: Option<String>,
    summary: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the periodic tasks sub-router: `/api/periodic/*`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_task))
        .route("/", get(list_tasks))
        .route("/{id}", get(get_task))
        .route("/{id}", put(update_task))
        .route("/{id}", delete(delete_task))
        .route("/{id}/enable", put(enable_task))
        .route("/{id}/disable", put(disable_task))
}

// ---------------------------------------------------------------------------
// POST /api/periodic/
// ---------------------------------------------------------------------------

/// Create a new periodic build task.
async fn create_task(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateTaskBody>,
) -> Result<(StatusCode, Json<PeriodicTaskResponse>), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:create") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:create",
        ));
    }
    if !user.has_cap("builds:create") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: builds:create",
        ));
    }

    // Validate cron expression.
    if croner::Cron::from_str(&body.cron_expr).is_err() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "invalid cron expression",
        ));
    }

    // Validate tag format placeholders.
    if let Err(unknown) = tag_format::validate_tag_format(&body.tag_format) {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            &format!("unknown tag format variables: {}", unknown.join(", ")),
        ));
    }

    // Validate descriptor is a JSON object.
    if !body.descriptor.is_object() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "descriptor must be a JSON object",
        ));
    }

    // Validate scopes at creation time so users cannot create tasks
    // targeting channels they lack access to (would silently fail at
    // trigger time).
    validate_descriptor_scopes(&state, &user, &body.descriptor).await?;

    let id = uuid::Uuid::new_v4().to_string();
    let descriptor_json = serde_json::to_string(&body.descriptor).map_err(|e| {
        tracing::error!("failed to serialize descriptor: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to serialize descriptor",
        )
    })?;

    db::periodic::insert_task(
        &state.pool,
        &id,
        &body.cron_expr,
        &body.tag_format,
        &descriptor_json,
        &body.priority,
        body.summary.as_deref(),
        &user.email,
    )
    .await
    .map_err(|e| {
        tracing::error!("failed to insert periodic task: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create periodic task",
        )
    })?;

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    let row = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task after insert: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "task not found after insert",
            )
        })?;

    tracing::info!(
        task_id = %id,
        "user {} created periodic task",
        user.email
    );

    Ok((StatusCode::CREATED, Json(task_to_response(row))))
}

// ---------------------------------------------------------------------------
// GET /api/periodic/
// ---------------------------------------------------------------------------

/// List all periodic build tasks.
async fn list_tasks(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PeriodicTaskResponse>>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:view",
        ));
    }

    let rows = db::periodic::list_tasks(&state.pool).await.map_err(|e| {
        tracing::error!("failed to list periodic tasks: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list periodic tasks",
        )
    })?;

    Ok(Json(rows.into_iter().map(task_to_response).collect()))
}

// ---------------------------------------------------------------------------
// GET /api/periodic/{id}
// ---------------------------------------------------------------------------

/// Get a single periodic build task by ID.
async fn get_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<PeriodicTaskResponse>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:view",
        ));
    }

    let row = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    Ok(Json(task_to_response(row)))
}

// ---------------------------------------------------------------------------
// PUT /api/periodic/{id}
// ---------------------------------------------------------------------------

/// Update a periodic build task. At least one field must be provided.
async fn update_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskBody>,
) -> Result<Json<PeriodicTaskResponse>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:manage",
        ));
    }

    // At least one field must be present.
    if body.cron_expr.is_none()
        && body.tag_format.is_none()
        && body.descriptor.is_none()
        && body.priority.is_none()
        && body.summary.is_none()
    {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "at least one field must be provided",
        ));
    }

    // If descriptor is being updated, require builds:create and
    // validate scopes against the new descriptor.
    if let Some(ref desc) = body.descriptor {
        if !user.has_cap("builds:create") {
            return Err(auth_error(
                StatusCode::FORBIDDEN,
                "missing required capability: builds:create",
            ));
        }
        validate_descriptor_scopes(&state, &user, desc).await?;
    }

    // Validate cron_expr if provided.
    if let Some(ref cron_expr) = body.cron_expr {
        if croner::Cron::from_str(cron_expr).is_err() {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                "invalid cron expression",
            ));
        }
    }

    // Validate tag_format if provided.
    if let Some(ref tf) = body.tag_format {
        if let Err(unknown) = tag_format::validate_tag_format(tf) {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown tag format variables: {}", unknown.join(", ")),
            ));
        }
    }

    // Validate descriptor if provided.
    if let Some(ref desc) = body.descriptor {
        if !desc.is_object() {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                "descriptor must be a JSON object",
            ));
        }
    }

    // Fetch the current row to merge updates.
    let current = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    // Merge fields.
    let new_cron_expr = body.cron_expr.unwrap_or(current.cron_expr);
    let new_tag_format = body.tag_format.unwrap_or(current.tag_format);
    let new_priority = body.priority.unwrap_or(current.priority);
    let new_summary = if body.summary.is_some() {
        body.summary
    } else {
        current.summary
    };
    let new_descriptor = if let Some(ref desc) = body.descriptor {
        serde_json::to_string(desc).map_err(|e| {
            tracing::error!("failed to serialize descriptor: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to serialize descriptor",
            )
        })?
    } else {
        current.descriptor
    };

    // Write back the full row. Clear retry state if the task was retrying.
    sqlx::query!(
        r#"UPDATE periodic_tasks
           SET cron_expr = ?, tag_format = ?, descriptor = ?, priority = ?,
               summary = ?, retry_count = 0, retry_at = NULL, last_error = NULL,
               updated_at = unixepoch()
           WHERE id = ?"#,
        new_cron_expr,
        new_tag_format,
        new_descriptor,
        new_priority,
        new_summary,
        id,
    )
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("failed to update periodic task '{id}': {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    let row = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}' after update: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "task not found after update",
            )
        })?;

    tracing::info!(
        task_id = %id,
        "user {} updated periodic task",
        user.email
    );

    Ok(Json(task_to_response(row)))
}

// ---------------------------------------------------------------------------
// DELETE /api/periodic/{id}
// ---------------------------------------------------------------------------

/// Delete a periodic build task.
async fn delete_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:manage",
        ));
    }

    let deleted = db::periodic::delete_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to delete periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if !deleted {
        return Err(auth_error(StatusCode::NOT_FOUND, "periodic task not found"));
    }

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    tracing::info!(
        task_id = %id,
        "user {} deleted periodic task",
        user.email
    );

    Ok(Json(
        serde_json::json!({"detail": format!("periodic task '{id}' deleted")}),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/periodic/{id}/enable
// ---------------------------------------------------------------------------

/// Enable a periodic build task. Resets retry state.
async fn enable_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:manage",
        ));
    }

    let updated = db::periodic::enable_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to enable periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if !updated {
        return Err(auth_error(StatusCode::NOT_FOUND, "periodic task not found"));
    }

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    tracing::info!(
        task_id = %id,
        "user {} enabled periodic task",
        user.email
    );

    Ok(Json(
        serde_json::json!({"detail": format!("periodic task '{id}' enabled")}),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/periodic/{id}/disable
// ---------------------------------------------------------------------------

/// Disable a periodic build task. Clears retry_at.
async fn disable_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:manage",
        ));
    }

    let updated = db::periodic::disable_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to disable periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if !updated {
        return Err(auth_error(StatusCode::NOT_FOUND, "periodic task not found"));
    }

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    tracing::info!(
        task_id = %id,
        "user {} disabled periodic task",
        user.email
    );

    Ok(Json(
        serde_json::json!({"detail": format!("periodic task '{id}' disabled")}),
    ))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract channel and repository scopes from a descriptor JSON and run
/// the same scope validation used by `submit_build`. This catches
/// permission issues at task creation/update time instead of silently
/// failing when the scheduler triggers the build.
async fn validate_descriptor_scopes(
    state: &AppState,
    user: &AuthUser,
    descriptor: &serde_json::Value,
) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    let mut scope_checks: Vec<(ScopeType, String)> = Vec::new();

    // Channel scope (if present in descriptor).
    if let Some(channel) = descriptor.get("channel").and_then(|v| v.as_str()) {
        if !channel.is_empty() {
            scope_checks.push((ScopeType::Channel, channel.to_string()));
        }
    }

    // Repository scopes from component repo overrides.
    if let Some(components) = descriptor.get("components").and_then(|v| v.as_array()) {
        for comp in components {
            if let Some(repo) = comp.get("repo").and_then(|v| v.as_str()) {
                scope_checks.push((ScopeType::Repository, repo.to_string()));
            }
        }
    }

    if !scope_checks.is_empty() {
        let scope_refs: Vec<(ScopeType, &str)> =
            scope_checks.iter().map(|(t, v)| (*t, v.as_str())).collect();
        user.require_scopes_all(&state.pool, &scope_refs).await?;
    }

    Ok(())
}
