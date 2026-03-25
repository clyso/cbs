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

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use cbsd_proto::{BuildDescriptor, BuildId, Priority};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, ScopeType, auth_error};
use crate::components;
use crate::db;
use crate::queue::QueuedBuild;
use crate::ws;

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

    // Repository scope checks from component repo overrides
    let mut scope_checks: Vec<(ScopeType, String)> = Vec::new();
    for comp in &body.descriptor.components {
        if let Some(ref repo) = comp.repo {
            scope_checks.push((ScopeType::Repository, repo.clone()));
        }
    }

    if !scope_checks.is_empty() {
        let scope_refs: Vec<(ScopeType, &str)> =
            scope_checks.iter().map(|(t, v)| (*t, v.as_str())).collect();
        user.require_scopes_all(&state.pool, &scope_refs).await?;
    }

    // Overwrite signed_off_by from the authenticated user record
    let mut descriptor = body.descriptor;
    descriptor.signed_off_by.user = user.name.clone();
    descriptor.signed_off_by.email = user.email.clone();

    // Resolve channel/type mapping and rewrite dst_image.name
    let user_record = db::users::get_user(&state.pool, &user.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get user record: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::INTERNAL_SERVER_ERROR, "user record not found"))?;

    let resolved = crate::channels::resolve_and_rewrite(&state.pool, &mut descriptor, &user_record)
        .await
        .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &e))?;

    let (build_id, pending_count) = insert_build_internal(
        &state,
        descriptor,
        &user.email,
        body.priority,
        None,
        Some(resolved.channel_id),
        Some(resolved.channel_type_id),
    )
    .await
    .map_err(|e| {
        tracing::error!("failed to submit build: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, &e)
    })?;

    let warning = if pending_count > 1 {
        Some(format!("{pending_count} build(s) in queue"))
    } else {
        None
    };

    tracing::info!(
        "user {} submitted build {build_id} (priority={:?})",
        user.email,
        body.priority,
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
// Shared build insertion (used by REST handler and scheduler trigger)
// ---------------------------------------------------------------------------

/// Insert a build into the DB, enqueue it, and attempt dispatch.
///
/// Returns `(build_id, pending_queue_count)`. Called by both the REST
/// `submit_build` handler (with `periodic_task_id = None`) and the
/// scheduler trigger (with `periodic_task_id = Some(id)`).
pub async fn insert_build_internal(
    state: &AppState,
    descriptor: BuildDescriptor,
    user_email: &str,
    priority: Priority,
    periodic_task_id: Option<&str>,
    channel_id: Option<i64>,
    channel_type_id: Option<i64>,
) -> Result<(i64, usize), String> {
    let descriptor_json = serde_json::to_string(&descriptor)
        .map_err(|e| format!("failed to serialize descriptor: {e}"))?;

    let priority_str = match priority {
        Priority::High => "high",
        Priority::Normal => "normal",
        Priority::Low => "low",
    };

    let build_id = db::builds::insert_build(
        &state.pool,
        &descriptor_json,
        user_email,
        priority_str,
        periodic_task_id,
        channel_id,
        channel_type_id,
    )
    .await
    .map_err(|e| format!("failed to insert build: {e}"))?;

    // Insert build_logs row at submission time so the SSE follow
    // endpoint can find it immediately (not only after dispatch).
    let log_path = format!("builds/{build_id}.log");
    db::builds::insert_build_log_row(&state.pool, build_id, &log_path)
        .await
        .map_err(|e| format!("failed to insert build_logs row: {e}"))?;

    let queued_at = chrono::Utc::now().timestamp();
    let queued_build = QueuedBuild {
        build_id: BuildId(build_id),
        priority,
        descriptor,
        user_email: user_email.to_string(),
        queued_at,
    };

    let pending_count;
    {
        let mut queue = state.queue.lock().await;
        queue.enqueue(queued_build);
        let (h, n, l) = queue.pending_counts();
        pending_count = h + n + l;
    }

    // Attempt immediate dispatch (non-blocking).
    let state_clone = state.clone();
    tokio::spawn(async move {
        if let Err(e) = ws::dispatch::try_dispatch(&state_clone).await {
            tracing::debug!("post-submit dispatch for build {build_id}: {e}");
        }
    });

    Ok((build_id, pending_count))
}

// ---------------------------------------------------------------------------
// GET /api/builds/
// ---------------------------------------------------------------------------

async fn list_builds(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListBuildsQuery>,
) -> Result<Json<Vec<db::builds::BuildListRecord>>, (StatusCode, Json<ErrorDetail>)> {
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
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list builds")
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

    match build.state.as_str() {
        "queued" => {
            // Remove from in-memory queue.
            {
                let mut queue = state.queue.lock().await;
                queue.remove_by_id(BuildId(id));
            }

            // Update DB state to revoked.
            db::builds::update_build_state(&state.pool, id, "revoked", None)
                .await
                .map_err(|e| {
                    tracing::error!("failed to update build {id} state to revoked: {e}");
                    auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
                })?;

            tracing::info!("user {} revoked queued build {id}", user.email);

            Ok(Json(
                serde_json::json!({"detail": format!("build {id} revoked")}),
            ))
        }
        "dispatched" | "started" => {
            // Send revoke to worker.
            ws::dispatch::send_build_revoke(&state, id)
                .await
                .map_err(|e| {
                    tracing::error!("failed to send revoke for build {id}: {e}");
                    auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to send revoke")
                })?;

            tracing::info!(
                "user {} requested revoke of {state_name} build {id}",
                user.email,
                state_name = build.state,
            );

            Ok(Json(
                serde_json::json!({"detail": format!("build {id} revoke sent — now revoking")}),
            ))
        }
        "revoking" => {
            tracing::info!(
                "user {} revoke of build {id} — already revoking",
                user.email
            );
            Ok(Json(
                serde_json::json!({"detail": format!("build {id} already revoking")}),
            ))
        }
        "success" | "failure" | "revoked" => Err(auth_error(
            StatusCode::CONFLICT,
            &format!("build already in terminal state: {}", build.state),
        )),
        _ => Err(auth_error(
            StatusCode::CONFLICT,
            &format!("unexpected build state: {}", build.state),
        )),
    }
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}/logs/tail?n=30
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LogsTailQuery {
    #[serde(default = "default_tail_n")]
    n: u32,
}

fn default_tail_n() -> u32 {
    30
}

/// Maximum number of lines the tail endpoint will return.
const MAX_TAIL_LINES: u32 = 10_000;

async fn logs_tail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(params): Query<LogsTailQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Cap n at MAX_TAIL_LINES.
    let n = params.n.min(MAX_TAIL_LINES) as usize;

    // Reuse the same :own/:any logic as get_build — if you can see the
    // build, you can see its logs.
    let build = db::builds::get_build(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get build {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "build not found"))?;

    let can_view_any = user.has_cap("builds:list:any");
    if !can_view_any {
        if !user.has_cap("builds:list:own") {
            return Err(auth_error(
                StatusCode::FORBIDDEN,
                "missing required capability: builds:list:own or builds:list:any",
            ));
        }
        if build.user_email != user.email {
            return Err(auth_error(StatusCode::NOT_FOUND, "build not found"));
        }
    }

    // Determine log file path.
    let log_path = state.config.log_dir.join(format!("builds/{id}.log"));

    // Read the file (if it exists).
    let contents = match tokio::fs::read_to_string(&log_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(
                build_id = id,
                log_path = %log_path.display(),
                "build log file not found"
            );
            return Err(auth_error(StatusCode::NOT_FOUND, "no logs yet"));
        }
        Err(e) => {
            tracing::error!("failed to read log file for build {id}: {e}");
            return Err(auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read log file",
            ));
        }
    };

    // Take last N lines.
    let all_lines: Vec<&str> = contents.lines().collect();
    let start = all_lines.len().saturating_sub(n);
    let tail: Vec<&str> = all_lines[start..].to_vec();

    Ok(Json(serde_json::json!({
        "build_id": id,
        "lines": tail,
        "total_lines": all_lines.len(),
        "returned": tail.len(),
    })))
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}/logs/follow
// ---------------------------------------------------------------------------

async fn logs_follow(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Same :own/:any gate as other log endpoints.
    let build = db::builds::get_build(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get build {id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database error".to_string(),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "build not found".to_string()))?;

    let can_view_any = user.has_cap("builds:list:any");
    if !can_view_any {
        if !user.has_cap("builds:list:own") {
            return Err((
                StatusCode::FORBIDDEN,
                "missing required capability: builds:list:own or builds:list:any".to_string(),
            ));
        }
        if build.user_email != user.email {
            return Err((StatusCode::NOT_FOUND, "build not found".to_string()));
        }
    }

    let last_event_id = crate::logs::sse::parse_last_event_id(&headers);
    crate::logs::sse::sse_follow(state, id, last_event_id).await
}

// ---------------------------------------------------------------------------
// GET /api/builds/{id}/logs
// ---------------------------------------------------------------------------

async fn logs_full(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorDetail>)> {
    tracing::debug!("logs_full: received request for build {id} logs");

    // Same :own/:any gate as other log endpoints.
    let build = db::builds::get_build(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get build {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "build not found"))?;

    let can_view_any = user.has_cap("builds:list:any");
    if !can_view_any {
        if !user.has_cap("builds:list:own") {
            return Err(auth_error(
                StatusCode::FORBIDDEN,
                "missing required capability: builds:list:own or builds:list:any",
            ));
        }
        if build.user_email != user.email {
            return Err(auth_error(StatusCode::NOT_FOUND, "build not found"));
        }
    }

    // Open the log file.
    let log_path = state.config.log_dir.join(format!("builds/{id}.log"));
    let file = tokio::fs::File::open(&log_path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            tracing::warn!(
                build_id = id,
                log_path = %log_path.display(),
                "build log file not found"
            );
            auth_error(StatusCode::NOT_FOUND, "no logs yet")
        } else {
            tracing::error!("failed to open log file for build {id}: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to open log file")
        }
    })?;

    // Stream the file as application/octet-stream.
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        body,
    ))
}
