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

//! Route handlers for `/api/admin/*`: entity management, worker registration.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde::Serialize;

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error, first_robot_forbidden_cap};
use crate::auth::token_cache;
use crate::db;
use crate::routes::permissions::ScopeBody;
use crate::routes::robots;

/// Build the admin sub-router: `/api/admin/*`.
pub fn router() -> Router<AppState> {
    let entity_router = Router::new()
        .route("/{email}/deactivate", put(deactivate_entity))
        .route("/{email}/activate", put(activate_entity))
        .route("/{email}/default-channel", put(set_entity_default_channel))
        .route("/{email}/roles", get(get_entity_roles))
        .route("/{email}/roles", put(replace_entity_roles))
        .route("/{email}/roles", post(add_entity_role))
        .route("/{email}/roles/{role}", delete(remove_entity_role));

    Router::new()
        .route("/entities", get(list_entities))
        .nest("/entity", entity_router)
        .nest("/robots", robots::router())
        .route("/queue", get(queue_status))
        .route("/workers", post(register_worker))
        .route("/workers/{id}", delete(deregister_worker))
        .route(
            "/workers/{id}/regenerate-token",
            post(regenerate_worker_token),
        )
}

// ---------------------------------------------------------------------------
// PUT /api/admin/entity/{email}/deactivate
// ---------------------------------------------------------------------------

/// Deactivate an entity: set active=0, bulk-revoke tokens + API keys, purge LRU
/// cache. Transactional with last-admin guard. Idempotent.
async fn deactivate_entity(
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
            serde_json::json!({"detail": format!("entity '{email}' already deactivated")}),
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

    // Purge LRU cache entries for this entity
    {
        let mut cache = state.token_cache.lock().await;
        cache.remove_by_owner(&email);
    }

    if target.is_robot {
        // Robot disable: leave tokens intact (tokens are revoked only on tombstone)
        tracing::info!("user {} disabled robot '{email}'", user.display_identity());
        return Ok(Json(serde_json::json!({
            "detail": format!("entity '{email}' deactivated"),
        })));
    }

    // Human: bulk-revoke PASETO tokens and API keys (outside transaction — idempotent)
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

    tracing::info!(
        "user {} deactivated entity '{email}' (revoked {tokens_revoked} tokens, {keys_revoked} API keys)",
        user.display_identity()
    );

    Ok(Json(serde_json::json!({
        "detail": format!("entity '{email}' deactivated"),
        "tokens_revoked": tokens_revoked,
        "api_keys_revoked": keys_revoked,
    })))
}

// ---------------------------------------------------------------------------
// PUT /api/admin/entity/{email}/activate
// ---------------------------------------------------------------------------

/// Reactivate an entity. Idempotent.
async fn activate_entity(
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

    // Robots require a non-revoked token to be re-activated (otherwise they
    // would be active but unable to authenticate). Return 400 rather than
    // 409 to signal a request-shape problem the caller must resolve (by
    // going through POST /api/admin/robots); 409 would imply "state
    // conflict, retry later", which is misleading here.
    if target.is_robot {
        let has_token = db::robots::has_non_revoked_token(&state.pool, &email)
            .await
            .map_err(|e| {
                tracing::error!("failed to check robot token for '{email}': {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;
        if !has_token {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                "cannot activate robot with no token — use POST /api/admin/robots to revive it with a new token",
            ));
        }
    }

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

    // Purge cache entries for this entity so the previous active=0 rejection
    // (AuthUser::load rejects deactivated users) does not survive in the LRU.
    // The next auth event reloads fresh state.
    {
        let mut cache = state.token_cache.lock().await;
        cache.remove_by_owner(&email);
    }

    tracing::info!(
        "user {} activated entity '{email}'",
        user.display_identity()
    );

    Ok(Json(
        serde_json::json!({"detail": format!("entity '{email}' activated")}),
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
        token_cache::generate_api_key_material()
            .await
            .map_err(|e| {
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
                if db::is_unique_violation(&e) {
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
        if db::is_unique_violation(&e) {
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
        let mut cache = state.token_cache.lock().await;
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
        token_cache::generate_api_key_material()
            .await
            .map_err(|e| {
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
        let mut cache = state.token_cache.lock().await;
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
// PUT /api/admin/entity/{email}/default-channel
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SetDefaultChannelBody {
    channel_id: i64,
}

/// Set an entity's default channel for build submission.
async fn set_entity_default_channel(
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

    // Verify target entity exists.
    let target = db::users::get_user(&state.pool, &email)
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

    // Robots cannot be assigned to a channel whose types use ${username}.
    if target.is_robot {
        let types = db::channels::list_types_for_channel(&state.pool, body.channel_id)
            .await
            .map_err(|e| {
                tracing::error!("failed to list types for channel {}: {e}", body.channel_id);
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        let bad_type = types
            .iter()
            .find(|t| crate::channels::prefix_template_contains_username(&t.prefix_template));
        if let Some(t) = bad_type {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "channel type '{}' uses the ${{username}} template — \
                     robot accounts cannot be assigned to such channels",
                    t.type_name
                ),
            ));
        }
    }

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
        "user {} set default channel for entity '{}' to {}",
        user.email,
        email,
        body.channel_id,
    );

    Ok(Json(serde_json::json!({
        "detail": format!("default channel set to {}", body.channel_id),
    })))
}

fn require_cap(user: &AuthUser, cap: &str) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap(cap) {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            &format!("missing required capability: {cap}"),
        ));
    }
    Ok(())
}

async fn last_admin_guard(pool: &sqlx::SqlitePool) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    let count = db::roles::count_active_wildcard_holders(pool)
        .await
        .map_err(|_| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check admin count",
            )
        })?;
    if count == 0 {
        return Err(auth_error(
            StatusCode::CONFLICT,
            "operation would remove the last admin — at least one active entity must hold the wildcard (*) capability",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entity role management types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct EntityRoleItem {
    role: String,
    scopes: Vec<ScopeBody>,
}

#[derive(Serialize)]
struct EntityWithRolesItem {
    email: String,
    name: String,
    active: bool,
    is_robot: bool,
    roles: Vec<EntityRoleItem>,
}

#[derive(Deserialize)]
struct ReplaceEntityRolesBody {
    roles: Vec<String>,
}

#[derive(Deserialize)]
struct AddEntityRoleBody {
    role: String,
}

// ---------------------------------------------------------------------------
// GET /api/admin/entities
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListEntitiesQuery {
    #[serde(rename = "type")]
    entity_type: Option<String>,
}

async fn list_entities(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ListEntitiesQuery>,
) -> Result<Json<Vec<EntityWithRolesItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:view")?;

    let filter = match query.entity_type.as_deref() {
        Some("user") => db::users::EntityFilter::User,
        Some("robot") => db::users::EntityFilter::Robot,
        Some("all") | None => db::users::EntityFilter::All,
        Some(other) => {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("invalid type filter '{other}': expected user, robot, or all"),
            ));
        }
    };

    let entities = db::users::list_entities_filtered(&state.pool, filter)
        .await
        .map_err(|e| {
            tracing::error!("failed to list entities: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list entities")
        })?;

    let mut result = Vec::with_capacity(entities.len());
    for row in entities {
        let email = row.email;
        let name = row.name;
        let active = row.active;
        let is_robot = row.is_robot;

        let entity_roles = db::roles::get_user_roles(&state.pool, &email)
            .await
            .map_err(|e| {
                tracing::error!("failed to get roles for entity '{email}': {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        let roles = entity_roles
            .into_iter()
            .map(|ur| EntityRoleItem {
                role: ur.role_name,
                scopes: ur.scopes.into_iter().map(Into::into).collect(),
            })
            .collect();

        result.push(EntityWithRolesItem {
            email,
            name,
            active,
            is_robot,
            roles,
        });
    }

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// GET /api/admin/entity/{email}/roles
// ---------------------------------------------------------------------------

async fn get_entity_roles(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
) -> Result<Json<Vec<EntityRoleItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:view")?;

    let entity_roles = db::roles::get_user_roles(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get roles for entity '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    Ok(Json(
        entity_roles
            .into_iter()
            .map(|ur| EntityRoleItem {
                role: ur.role_name,
                scopes: ur.scopes.into_iter().map(Into::into).collect(),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/admin/entity/{email}/roles
// ---------------------------------------------------------------------------

async fn replace_entity_roles(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
    Json(body): Json<ReplaceEntityRolesBody>,
) -> Result<Json<Vec<EntityRoleItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    // Refuse to assign roles holding forbidden caps to robot targets. The
    // auth-time strip still guards the cap surface at request time; this
    // assignment-time reject is defense in depth.
    let target_is_robot = db::users::get_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up entity '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .map(|u| u.is_robot)
        .unwrap_or(false);

    for role_name in &body.roles {
        if db::roles::get_role(&state.pool, role_name)
            .await
            .map_err(|e| {
                tracing::error!("failed to get role '{role_name}': {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?
            .is_none()
        {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("role '{role_name}' does not exist"),
            ));
        }

        if target_is_robot {
            let caps = db::roles::get_role_caps(&state.pool, role_name)
                .await
                .map_err(|e| {
                    tracing::error!("failed to load caps for role '{role_name}': {e}");
                    auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
                })?;

            if let Some(forbidden) = first_robot_forbidden_cap(&caps) {
                return Err(auth_error(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "role '{role_name}' carries cap '{forbidden}' which robots cannot hold"
                    ),
                ));
            }
        }
    }

    let role_refs: Vec<&str> = body.roles.iter().map(String::as_str).collect();
    db::roles::set_user_roles(&state.pool, &email, &role_refs)
        .await
        .map_err(|e| {
            tracing::error!("failed to set roles for entity '{email}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set entity roles",
            )
        })?;

    last_admin_guard(&state.pool).await?;

    tracing::info!("user {} replaced roles for entity '{email}'", user.email);

    let entity_roles = db::roles::get_user_roles(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get roles for entity '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    Ok(Json(
        entity_roles
            .into_iter()
            .map(|ur| EntityRoleItem {
                role: ur.role_name,
                scopes: ur.scopes.into_iter().map(Into::into).collect(),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// POST /api/admin/entity/{email}/roles
// ---------------------------------------------------------------------------

async fn add_entity_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
    Json(body): Json<AddEntityRoleBody>,
) -> Result<(StatusCode, Json<EntityRoleItem>), (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    db::roles::get_role(&state.pool, &body.role)
        .await
        .map_err(|e| {
            tracing::error!("failed to get role '{}': {e}", body.role);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::BAD_REQUEST,
                &format!("role '{}' does not exist", body.role),
            )
        })?;

    // Assignment-time forbidden-cap reject for robot targets.
    let target_is_robot = db::users::get_user(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up entity '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .map(|u| u.is_robot)
        .unwrap_or(false);

    if target_is_robot {
        let caps = db::roles::get_role_caps(&state.pool, &body.role)
            .await
            .map_err(|e| {
                tracing::error!("failed to load caps for role '{}': {e}", body.role);
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        if let Some(forbidden) = first_robot_forbidden_cap(&caps) {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "role '{}' carries cap '{forbidden}' which robots cannot hold",
                    body.role
                ),
            ));
        }
    }

    db::roles::add_user_role(&state.pool, &email, &body.role)
        .await
        .map_err(|e| {
            tracing::error!(
                "failed to add role '{}' to entity '{email}': {e}",
                body.role
            );
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to add entity role",
            )
        })?;

    let scopes = db::roles::get_role_scopes(&state.pool, &body.role)
        .await
        .map_err(|e| {
            tracing::error!("failed to get scopes for role '{}': {e}", body.role);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    tracing::info!(
        "user {} added role '{}' to entity '{email}'",
        user.email,
        body.role
    );

    Ok((
        StatusCode::CREATED,
        Json(EntityRoleItem {
            role: body.role,
            scopes: scopes.into_iter().map(Into::into).collect(),
        }),
    ))
}

// ---------------------------------------------------------------------------
// DELETE /api/admin/entity/{email}/roles/{role}
// ---------------------------------------------------------------------------

async fn remove_entity_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path((email, role)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    let removed = db::roles::remove_user_role(&state.pool, &email, &role)
        .await
        .map_err(|e| {
            tracing::error!(
                "failed to remove role '{}' from entity '{email}': {e}",
                role
            );
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove entity role",
            )
        })?;

    if !removed {
        return Err(auth_error(
            StatusCode::NOT_FOUND,
            "role assignment not found",
        ));
    }

    last_admin_guard(&state.pool).await?;

    tracing::info!(
        "user {} removed role '{}' from entity '{email}'",
        user.email,
        role
    );

    Ok(Json(
        serde_json::json!({"detail": format!("role '{role}' removed from entity '{email}'")}),
    ))
}

#[cfg(test)]
mod handler_tests {
    use super::*;
    use crate::routes::test_support::{auth_user, test_app_state, test_pool};

    #[tokio::test]
    async fn list_entities_rejects_unknown_type_filter_with_400() {
        let pool = test_pool().await;
        let state = test_app_state(pool);
        let caller = auth_user("admin@example.com", "Admin", false, &["permissions:view"]);
        let query = ListEntitiesQuery {
            entity_type: Some("garbage".to_string()),
        };

        match list_entities(State(state), caller, Query(query)).await {
            Err((status, _)) => assert_eq!(status, StatusCode::BAD_REQUEST),
            Ok(_) => panic!("unknown ?type= value must return 400"),
        }
    }

    #[tokio::test]
    async fn list_entities_accepts_absent_type_filter_as_all() {
        let pool = test_pool().await;
        let state = test_app_state(pool);
        let caller = auth_user("admin@example.com", "Admin", false, &["permissions:view"]);
        let query = ListEntitiesQuery { entity_type: None };

        match list_entities(State(state), caller, Query(query)).await {
            Ok(Json(rows)) => assert!(rows.is_empty(), "empty DB yields empty entity list"),
            Err((status, _)) => panic!("expected Ok, got {status}"),
        }
    }
}
