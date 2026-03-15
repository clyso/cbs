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

//! Route handlers for `/api/builds/*`: build submission, listing, revocation.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use cbsd_proto::{BuildDescriptor, BuildId, Priority};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::auth::extractors::{auth_error, AuthUser, ErrorDetail, ScopeType};
use crate::components;
use crate::db;
use crate::queue::QueuedBuild;

/// Build the builds sub-router: `/api/builds/*`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(submit_build))
        .route("/", get(list_builds))
        .route("/{id}", get(get_build))
        .route("/{id}", delete(revoke_build))
        .route("/{id}/logs/tail", get(logs_tail))
        .route("/{id}/logs/follow", get(logs_follow))
        .route("/{id}/logs", get(logs_full))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SubmitBuildBody {
    descriptor: BuildDescriptor,
    #[serde(default)]
    priority: Priority,
}

#[derive(Serialize)]
struct SubmitBuildResponse {
    id: i64,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

#[derive(Deserialize)]
struct ListBuildsQuery {
    user: Option<String>,
    state: Option<String>,
}

// ---------------------------------------------------------------------------
// POST /api/builds/
// ---------------------------------------------------------------------------

async fn submit_build(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<SubmitBuildBody>,
) -> Result<(StatusCode, Json<SubmitBuildResponse>), (StatusCode, Json<ErrorDetail>)> {
    // Check capability
    if !user.has_cap("builds:create") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: builds:create",
        ));
    }

    // Validate component names
    for comp in &body.descriptor.components {
        if !components::validate_component_name(&state.components, &comp.name) {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown component: {}", comp.name),
            ));
        }
    }

    // Build scope checks from descriptor
    let mut scope_checks: Vec<(ScopeType, String)> = Vec::new();

    // Channel scope
    scope_checks.push((ScopeType::Channel, body.descriptor.channel.clone()));

    // Registry scope from dst_image
    if let Some(host) = body.descriptor.registry_host() {
        scope_checks.push((ScopeType::Registry, host.to_string()));
    }

    // Repository scopes from component repo overrides
    for comp in &body.descriptor.components {
        if let Some(ref repo) = comp.repo {
            scope_checks.push((ScopeType::Repository, repo.clone()));
        }
    }

    // Convert to borrowed slice for require_scopes_all
    let scope_refs: Vec<(ScopeType, &str)> = scope_checks
        .iter()
        .map(|(t, v)| (*t, v.as_str()))
        .collect();

    user.require_scopes_all(&state.pool, &scope_refs).await?;

    // Overwrite signed_off_by from the authenticated user record
    let mut descriptor = body.descriptor;
    descriptor.signed_off_by.user = user.name.clone();
    descriptor.signed_off_by.email = user.email.clone();

    // Serialize descriptor to JSON for storage
    let descriptor_json = serde_json::to_string(&descriptor).map_err(|e| {
        tracing::error!("failed to serialize descriptor: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to serialize build descriptor",
        )
    })?;

    // Map priority to DB string
    let priority_str = match body.priority {
        Priority::High => "high",
        Priority::Normal => "normal",
        Priority::Low => "low",
    };

    // Insert into database
    let build_id =
        db::builds::insert_build(&state.pool, &descriptor_json, &user.email, priority_str)
            .await
            .map_err(|e| {
                tracing::error!("failed to insert build: {e}");
                auth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create build record",
                )
            })?;

    // Enqueue in the in-memory queue
    let queued_at = chrono::Utc::now().timestamp();
    let queued_build = QueuedBuild {
        build_id: BuildId(build_id),
        priority: body.priority,
        descriptor,
        user_email: user.email.clone(),
        queued_at,
    };

    let warning;
    {
        let mut queue = state.queue.lock().await;
        queue.enqueue(queued_build);
        let (h, n, l) = queue.pending_counts();
        let total = h + n + l;
        // Warn if there are other builds ahead (workers not dispatching yet)
        warning = if total > 1 {
            Some(format!(
                "{} build(s) in queue — full dispatch available in Phase 4",
                total
            ))
        } else {
            None
        };
    }

    tracing::info!(
        "user {} submitted build {build_id} (priority={priority_str})",
        user.email
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitBuildResponse {
            id: build_id,
            state: "queued".to_string(),
            warning,
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/builds/
// ---------------------------------------------------------------------------

async fn list_builds(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListBuildsQuery>,
) -> Result<Json<Vec<db::builds::BuildRecord>>, (StatusCode, Json<ErrorDetail>)> {
    let can_list_any = user.has_cap("builds:list:any");
    let can_list_own = user.has_cap("builds:list:own");

    if !can_list_any && !can_list_own {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: builds:list:own or builds:list:any",
        ));
    }

    // Determine user filter
    let user_filter = if can_list_any {
        // Admin can filter by user, or see all
        params.user.as_deref()
    } else {
        // Non-admin can only see their own builds
        Some(user.email.as_str())
    };

    let builds = db::builds::list_builds(&state.pool, user_filter, params.state.as_deref())
        .await
        .map_err(|e| {
            tracing::error!("failed to list builds: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list builds",
            )
        })?;

    Ok(Json(builds))
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}
// ---------------------------------------------------------------------------

async fn get_build(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<db::builds::BuildRecord>, (StatusCode, Json<ErrorDetail>)> {
    let build = db::builds::get_build(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get build {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "build not found"))?;

    // Ownership check: if user only has :own, verify email matches
    let can_view_any = user.has_cap("builds:list:any");
    if !can_view_any {
        let can_view_own = user.has_cap("builds:list:own");
        if !can_view_own {
            return Err(auth_error(
                StatusCode::FORBIDDEN,
                "missing required capability: builds:list:own or builds:list:any",
            ));
        }
        if build.user_email != user.email {
            return Err(auth_error(StatusCode::NOT_FOUND, "build not found"));
        }
    }

    Ok(Json(build))
}

// ---------------------------------------------------------------------------
// DELETE /api/builds/{id}
// ---------------------------------------------------------------------------

async fn revoke_build(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Check revoke capability
    let can_revoke_any = user.has_cap("builds:revoke:any");
    let can_revoke_own = user.has_cap("builds:revoke:own");

    if !can_revoke_any && !can_revoke_own {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: builds:revoke:own or builds:revoke:any",
        ));
    }

    // Load the build
    let build = db::builds::get_build(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get build {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "build not found"))?;

    // Ownership check
    if !can_revoke_any && build.user_email != user.email {
        return Err(auth_error(StatusCode::NOT_FOUND, "build not found"));
    }

    // Only QUEUED builds can be revoked in Phase 3.
    // DISPATCHED/STARTED handling deferred to Phase 4.
    if build.state != "queued" {
        return Err(auth_error(
            StatusCode::CONFLICT,
            "build not in queued state — revocation of dispatched/started builds available in Phase 4",
        ));
    }

    // Remove from in-memory queue
    {
        let mut queue = state.queue.lock().await;
        queue.remove_by_id(BuildId(id));
    }

    // Update DB state to revoked
    db::builds::update_build_state(&state.pool, id, "revoked", None)
        .await
        .map_err(|e| {
            tracing::error!("failed to update build {id} state to revoked: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    tracing::info!("user {} revoked build {id}", user.email);

    Ok(Json(
        serde_json::json!({"detail": format!("build {id} revoked")}),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}/logs/tail
// ---------------------------------------------------------------------------

async fn logs_tail(
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    let _ = id;
    Err(auth_error(StatusCode::NOT_FOUND, "no logs yet"))
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}/logs/follow
// ---------------------------------------------------------------------------

async fn logs_follow(
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    let _ = id;
    Err(auth_error(StatusCode::NOT_FOUND, "no logs yet"))
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}/logs
// ---------------------------------------------------------------------------

async fn logs_full(
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    let _ = id;
    Err(auth_error(StatusCode::NOT_FOUND, "no logs yet"))
}
