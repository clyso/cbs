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

//! Route handlers for `/api/admin/*`: user management and worker registration.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde::Serialize;

use crate::app::AppState;
use crate::auth::api_keys;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error};
use crate::db;

/// Build the admin sub-router: `/api/admin/*`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{email}/deactivate", put(deactivate_user))
        .route("/users/{email}/activate", put(activate_user))
        .route("/queue", get(queue_status))
        .route("/workers", post(register_worker))
        .route("/workers/{id}", delete(deregister_worker))
        .route(
            "/workers/{id}/regenerate-token",
            post(regenerate_worker_token),
        )
        .route(
            "/users/{email}/default-channel",
            put(set_user_default_channel),
        )
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

    sqlx::query!(
        "UPDATE users SET active = 0, updated_at = unixepoch() WHERE email = ?",
        email,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("failed to deactivate user '{email}': {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // Last-admin guard within transaction: count remaining active wildcard holders
    let count = db::roles::count_active_wildcard_holders_tx(&mut tx)
        .await
        .map_err(|e| {
            tracing::error!("failed to check admin count: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;
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
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke tokens")
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

    sqlx::query!(
        "UPDATE users SET active = 1, updated_at = unixepoch() WHERE email = ?",
        email,
    )
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

// ---------------------------------------------------------------------------
// Worker registration types
// ---------------------------------------------------------------------------

/// Worker name validation: `[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}`.
fn is_valid_worker_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Deserialize)]
struct RegisterWorkerBody {
    name: String,
    arch: String,
}

#[derive(Serialize)]
struct RegisterWorkerResponse {
    worker_id: String,
    name: String,
    arch: String,
    worker_token: String,
}

fn build_worker_token(worker_id: &str, name: &str, api_key: &str, arch: &str) -> String {
    let payload = cbsd_proto::WorkerToken {
        worker_id: worker_id.to_string(),
        worker_name: name.to_string(),
        api_key: api_key.to_string(),
        arch: arch.to_string(),
    };
    let json = serde_json::to_string(&payload).expect("WorkerToken is always serializable");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
}

// ---------------------------------------------------------------------------
// POST /api/admin/workers
// ---------------------------------------------------------------------------

/// Register a new worker: create API key + worker row in one transaction,
/// return a base64url worker token.
///
/// SECURITY: The response body contains the plaintext API key (inside the
/// worker token). Ensure no response-body logging middleware captures it.
async fn register_worker(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<RegisterWorkerBody>,
) -> Result<(StatusCode, Json<RegisterWorkerResponse>), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("workers:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: workers:manage",
        ));
    }

    if !is_valid_worker_name(&body.name) {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "invalid worker name: must match [a-zA-Z0-9][a-zA-Z0-9_-]{0,63}",
        ));
    }

    // Validate arch
    if body.arch != "x86_64" && body.arch != "aarch64" {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "invalid arch: must be x86_64 or aarch64",
        ));
    }

    let worker_id = uuid::Uuid::new_v4().to_string();

    // Generate key material BEFORE the transaction (argon2 is CPU-bound)
    let (plaintext_key, prefix, hash) =
        api_keys::generate_api_key_material().await.map_err(|e| {
            tracing::error!("failed to generate API key material: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate API key",
            )
        })?;

    // Atomic transaction: insert API key + worker row
    let mut tx = state.pool.begin().await.map_err(|e| {
        tracing::error!("failed to begin transaction: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let api_key_name = format!("worker:{}", body.name);
    let api_key_id =
        db::api_keys::insert_api_key_in_tx(&mut tx, &api_key_name, &user.email, &hash, &prefix)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    return auth_error(StatusCode::CONFLICT, "worker name already exists");
                }
                tracing::error!("failed to insert API key: {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

    db::workers::insert_worker(
        &mut tx,
        &worker_id,
        &body.name,
        &body.arch,
        api_key_id,
        &user.email,
    )
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            return auth_error(StatusCode::CONFLICT, "worker name already exists");
        }
        tracing::error!("failed to insert worker: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!("failed to commit worker registration: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let token = build_worker_token(&worker_id, &body.name, &plaintext_key, &body.arch);

    tracing::info!(
        worker_id = %worker_id,
        name = %body.name,
        arch = %body.arch,
        "user {} registered worker '{}'",
        user.email,
        body.name,
    );

    Ok((
        StatusCode::CREATED,
        Json(RegisterWorkerResponse {
            worker_id,
            name: body.name,
            arch: body.arch,
            worker_token: token,
        }),
    ))
}

// ---------------------------------------------------------------------------
// DELETE /api/admin/workers/{id}
// ---------------------------------------------------------------------------

/// Deregister a worker: revoke its API key, purge cache, delete DB row,
/// and force-disconnect the live WebSocket connection (re-queuing any
/// in-flight build via the dead-worker resolution mechanism).
async fn deregister_worker(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("workers:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: workers:manage",
        ));
    }

    let worker = db::workers::get_worker_by_id(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up worker '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "worker not found"))?;

    // Atomic: revoke API key + delete worker row in one transaction
    let mut tx = state.pool.begin().await.map_err(|e| {
        tracing::error!("failed to begin transaction: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let api_key_id = worker.api_key_id;
    sqlx::query!(
        "UPDATE api_keys SET revoked = 1 WHERE id = ? AND revoked = 0",
        api_key_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("failed to revoke API key for worker '{}': {e}", worker.name);
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to revoke API key",
        )
    })?;

    sqlx::query!("DELETE FROM workers WHERE id = ?", id,)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("failed to delete worker '{}': {e}", worker.name);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    tx.commit().await.map_err(|e| {
        tracing::error!("failed to commit worker deregistration: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // Purge from LRU cache (after commit)
    if let Some(prefix) = db::api_keys::get_key_prefix_by_id(&state.pool, worker.api_key_id)
        .await
        .ok()
        .flatten()
    {
        let mut cache = state.api_key_cache.lock().await;
        cache.remove_by_prefix(&prefix);
    }

    // Force-disconnect: find connection by registered_worker_id, remove from
    // queue map, then (after releasing queue lock) remove sender and re-queue
    // any in-flight build.
    force_disconnect_worker(&state, &id).await;

    tracing::info!(
        "user {} deregistered worker '{}' (id={})",
        user.email,
        worker.name,
        id,
    );

    Ok(Json(serde_json::json!({
        "detail": format!("worker '{}' deregistered", worker.name),
        "api_key_revoked": true,
    })))
}

// ---------------------------------------------------------------------------
// POST /api/admin/workers/{id}/regenerate-token
// ---------------------------------------------------------------------------

/// Rotate a worker's API key: revoke old key, create new one, return new
/// worker token. Crash-safe: insert new → update FK → revoke old → commit.
/// Force-disconnects the worker so it must reconnect with the new key.
async fn regenerate_worker_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<RegisterWorkerResponse>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("workers:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: workers:manage",
        ));
    }

    let worker = db::workers::get_worker_by_id(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up worker '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "worker not found"))?;

    let old_api_key_id = worker.api_key_id;

    // Generate new key material BEFORE the transaction
    let (plaintext_key, prefix, hash) =
        api_keys::generate_api_key_material().await.map_err(|e| {
            tracing::error!("failed to generate API key material: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate API key",
            )
        })?;

    // Atomic: insert new key → update FK → revoke old key
    let mut tx = state.pool.begin().await.map_err(|e| {
        tracing::error!("failed to begin transaction: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    let api_key_name = format!("worker:{}", worker.name);
    let new_api_key_id =
        db::api_keys::insert_api_key_in_tx(&mut tx, &api_key_name, &user.email, &hash, &prefix)
            .await
            .map_err(|e| {
                tracing::error!("failed to insert new API key: {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

    db::workers::update_api_key_id(&mut tx, &id, new_api_key_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to update worker API key: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    // Revoke old key inside the transaction
    sqlx::query!(
        "UPDATE api_keys SET revoked = 1 WHERE id = ? AND revoked = 0",
        old_api_key_id,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("failed to revoke old API key: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!("failed to commit token regeneration: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // Purge old key from LRU cache (after commit)
    if let Some(old_prefix) = db::api_keys::get_key_prefix_by_id(&state.pool, old_api_key_id)
        .await
        .ok()
        .flatten()
    {
        let mut cache = state.api_key_cache.lock().await;
        cache.remove_by_prefix(&old_prefix);
    }

    // Force-disconnect so the worker must reconnect with the new key.
    force_disconnect_worker(&state, &id).await;

    let token = build_worker_token(&id, &worker.name, &plaintext_key, &worker.arch);

    tracing::info!(
        "user {} regenerated token for worker '{}' (id={})",
        user.email,
        worker.name,
        id,
    );

    Ok(Json(RegisterWorkerResponse {
        worker_id: id,
        name: worker.name,
        arch: worker.arch,
        worker_token: token,
    }))
}

/// Force-disconnect a worker by its registered UUID.
///
/// Sequence (avoids deadlock — `handle_worker_dead` re-acquires queue mutex):
/// 1. Lock queue → scan for connection by `registered_worker_id` → extract
///    connection_id → remove entry → release lock.
/// 2. Remove from `worker_senders` (drops sender, closes socket).
/// 3. Call `handle_worker_dead` to re-queue any in-flight build.
async fn force_disconnect_worker(state: &AppState, registered_worker_id: &str) {
    let connection_id = {
        let mut queue = state.queue.lock().await;
        let found = queue
            .workers
            .iter()
            .find(|(_, ws)| ws.registered_worker_id() == Some(registered_worker_id))
            .map(|(cid, _)| cid.clone());

        if let Some(cid) = &found {
            queue.workers.remove(cid.as_str());
        }
        found
    };
    // Queue lock released.

    if let Some(cid) = connection_id {
        // Drop the sender — closes the socket, triggers cleanup_worker which
        // finds no queue entry and bails.
        {
            let mut senders = state.worker_senders.lock().await;
            senders.remove(&cid);
        }

        // Re-queue any in-flight build.
        crate::ws::handler::handle_worker_dead(state, &cid).await;

        tracing::info!(
            connection_id = %cid,
            registered_worker_id = %registered_worker_id,
            "force-disconnected worker"
        );
    }
}

// ---------------------------------------------------------------------------
// PUT /api/admin/users/{email}/default-channel
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SetDefaultChannelBody {
    channel_id: i64,
}

/// Set a user's default channel for build submission.
async fn set_user_default_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
    Json(body): Json<SetDefaultChannelBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("permissions:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: permissions:manage",
        ));
    }

    // Verify target user exists.
    db::users::get_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "user not found"))?;

    // Verify channel exists.
    db::channels::get_channel_by_id(&state.pool, body.channel_id)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up channel {}: {e}", body.channel_id);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "channel not found"))?;

    db::users::set_default_channel(&state.pool, &email, Some(body.channel_id))
        .await
        .map_err(|e| {
            tracing::error!("failed to set default channel for '{email}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set default channel",
            )
        })?;

    tracing::info!(
        "user {} set default channel for '{}' to {}",
        user.email,
        email,
        body.channel_id,
    );

    Ok(Json(serde_json::json!({
        "detail": format!("default channel set to {}", body.channel_id),
    })))
}

/// Check if a sqlx error is a UNIQUE constraint violation.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        // SQLite error code 2067 = SQLITE_CONSTRAINT_UNIQUE
        db_err.code().is_some_and(|c| c == "2067")
    } else {
        false
    }
}
