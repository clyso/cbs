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

//! Route handlers for `/api/admin/*`: user activation/deactivation.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};

use crate::app::AppState;
use crate::auth::extractors::{auth_error, AuthUser, ErrorDetail};
use crate::db;

/// Build the admin sub-router: `/api/admin/*`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{email}/deactivate", put(deactivate_user))
        .route("/users/{email}/activate", put(activate_user))
        .route("/queue", get(queue_status))
}

// ---------------------------------------------------------------------------
// PUT /api/admin/users/{email}/deactivate
// ---------------------------------------------------------------------------

/// Deactivate a user: set active=0, bulk-revoke tokens + API keys, purge LRU
/// cache. Transactional with last-admin guard. Idempotent.
async fn deactivate_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("permissions:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: permissions:manage",
        ));
    }

    // Verify target user exists
    let target = db::users::get_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "user not found"))?;

    // Idempotent: if already deactivated, return success
    if !target.active {
        return Ok(Json(
            serde_json::json!({"detail": format!("user '{email}' already deactivated")}),
        ));
    }

    // Transactional: deactivate + last-admin guard
    let mut tx = state.pool.begin().await.map_err(|e| {
        tracing::error!("failed to begin transaction: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    sqlx::query("UPDATE users SET active = 0, updated_at = unixepoch() WHERE email = ?")
        .bind(&email)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("failed to deactivate user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    // Last-admin guard within transaction: count remaining active wildcard holders
    let row = sqlx::query(
        "SELECT COUNT(DISTINCT u.email) as cnt
         FROM users u
         JOIN user_roles ur ON u.email = ur.user_email
         JOIN role_caps rc ON ur.role_name = rc.role_name
         WHERE u.active = 1 AND rc.cap = '*'",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("failed to check admin count: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let count: i64 = sqlx::Row::get(&row, "cnt");
    if count == 0 {
        // Rollback: transaction will be dropped without commit
        return Err(auth_error(
            StatusCode::CONFLICT,
            "cannot deactivate the last admin — at least one active user must hold the wildcard (*) capability",
        ));
    }

    tx.commit().await.map_err(|e| {
        tracing::error!("failed to commit deactivation for '{email}': {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // Bulk-revoke tokens and API keys (outside transaction — idempotent)
    let tokens_revoked = db::tokens::revoke_all_for_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to revoke tokens for '{email}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to revoke tokens",
            )
        })?;

    let keys_revoked = db::api_keys::revoke_all_api_keys_for_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to revoke API keys for '{email}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to revoke API keys",
            )
        })?;

    // Purge LRU cache entries for this user
    {
        let mut cache = state.api_key_cache.lock().await;
        cache.remove_by_owner(&email);
    }

    tracing::info!(
        "user {} deactivated user '{email}' (revoked {tokens_revoked} tokens, {keys_revoked} API keys)",
        user.email
    );

    Ok(Json(serde_json::json!({
        "detail": format!("user '{email}' deactivated"),
        "tokens_revoked": tokens_revoked,
        "api_keys_revoked": keys_revoked,
    })))
}

// ---------------------------------------------------------------------------
// PUT /api/admin/users/{email}/activate
// ---------------------------------------------------------------------------

/// Reactivate a user. Idempotent.
async fn activate_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("permissions:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: permissions:manage",
        ));
    }

    // Verify target user exists
    db::users::get_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "user not found"))?;

    sqlx::query("UPDATE users SET active = 1, updated_at = unixepoch() WHERE email = ?")
        .bind(&email)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("failed to activate user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    tracing::info!("user {} activated user '{email}'", user.email);

    Ok(Json(
        serde_json::json!({"detail": format!("user '{email}' activated")}),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/admin/queue
// ---------------------------------------------------------------------------

/// Return the number of pending builds per priority lane.
async fn queue_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("admin:queue:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: admin:queue:view",
        ));
    }

    let (high, normal, low) = {
        let queue = state.queue.lock().await;
        queue.pending_counts()
    };

    Ok(Json(serde_json::json!({
        "high": high,
        "normal": normal,
        "low": low,
    })))
}
