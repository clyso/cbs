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

//! Route handlers for `/api/permissions/*`: roles and user-role management.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error};
use crate::db;

/// Known capabilities (validated at the API layer).
const KNOWN_CAPS: &[&str] = &[
    "builds:create",
    "builds:revoke:own",
    "builds:revoke:any",
    "builds:list:own",
    "builds:list:any",
    "admin:queue:view",
    "permissions:view",
    "permissions:manage",
    "apikeys:create:own",
    "robots:manage",
    "robots:view",
    "workers:view",
    "workers:manage",
    "periodic:create",
    "periodic:view",
    "periodic:manage",
    "channels:manage",
    "channels:view",
    "*",
];

/// Capabilities that require scopes on assignment.
const SCOPE_DEPENDENT_CAPS: &[&str] = &["builds:create"];

/// Build the permissions sub-router: `/api/permissions/*`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/roles", get(list_roles))
        .route("/roles", post(create_role))
        .route("/roles/{name}", get(get_role))
        .route("/roles/{name}", put(update_role))
        .route("/roles/{name}", delete(delete_role))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateRoleBody {
    name: String,
    description: Option<String>,
    caps: Vec<String>,
    #[serde(default)]
    scopes: Vec<ScopeBody>,
}

#[derive(Serialize)]
struct RoleResponse {
    name: String,
    description: String,
    builtin: bool,
    caps: Vec<String>,
    scopes: Vec<ScopeBody>,
    created_at: i64,
}

#[derive(Serialize)]
struct RoleListItem {
    name: String,
    description: String,
    builtin: bool,
    created_at: i64,
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct ScopeBody {
    #[serde(rename = "type")]
    scope_type: String,
    pattern: String,
}

impl From<db::roles::ScopeEntry> for ScopeBody {
    fn from(s: db::roles::ScopeEntry) -> Self {
        Self {
            scope_type: s.scope_type,
            pattern: s.pattern,
        }
    }
}

impl From<ScopeBody> for db::roles::ScopeEntry {
    fn from(s: ScopeBody) -> Self {
        Self {
            scope_type: s.scope_type,
            pattern: s.pattern,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_caps(caps: &[String]) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    for cap in caps {
        if !KNOWN_CAPS.contains(&cap.as_str()) {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown capability: {cap}"),
            ));
        }
    }
    Ok(())
}

/// Check if a role's capabilities include any scope-dependent cap.
/// Roles with `*` (admin wildcard) are global — no scopes required.
fn role_is_scope_dependent(caps: &[String]) -> bool {
    if caps.iter().any(|c| c == "*") {
        return false;
    }
    caps.iter()
        .any(|c| SCOPE_DEPENDENT_CAPS.contains(&c.as_str()))
}

/// Check whether removing something would leave zero active wildcard holders.
/// Returns `Err(409)` if the guard fires.
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
            "operation would remove the last admin — at least one active user must hold the wildcard (*) capability",
        ));
    }
    Ok(())
}

/// Known scope types (validated at the API layer to return a clean 400
/// instead of letting the DB CHECK constraint produce a 500).
const KNOWN_SCOPE_TYPES: &[&str] = &["channel", "registry", "repository"];

/// Validate scope entries: check type is known and channel patterns
/// contain `/` to enforce the `channel/type` format.
fn validate_scopes(scopes: &[ScopeBody]) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    for scope in scopes {
        if !KNOWN_SCOPE_TYPES.contains(&scope.scope_type.as_str()) {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "unknown scope type '{}': must be one of channel, registry, repository",
                    scope.scope_type
                ),
            ));
        }
        if scope.scope_type == "channel" && !scope.pattern.contains('/') && scope.pattern != "*" {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "channel scope pattern '{}' must contain '/' (e.g. 'ces/dev' or 'ces/*')",
                    scope.pattern
                ),
            ));
        }
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// GET /api/permissions/roles
// ---------------------------------------------------------------------------

async fn list_roles(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<RoleListItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:view")?;

    let roles = db::roles::list_roles(&state.pool).await.map_err(|e| {
        tracing::error!("failed to list roles: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list roles")
    })?;

    Ok(Json(
        roles
            .into_iter()
            .map(|r| RoleListItem {
                name: r.name,
                description: r.description,
                builtin: r.builtin,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// POST /api/permissions/roles
// ---------------------------------------------------------------------------

async fn create_role(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateRoleBody>,
) -> Result<(StatusCode, Json<RoleResponse>), (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;
    validate_caps(&body.caps)?;
    validate_scopes(&body.scopes)?;

    // Scope-dependent validation: roles with builds:create (etc.) need scopes
    if role_is_scope_dependent(&body.caps) && body.scopes.is_empty() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "role contains scope-dependent capabilities and requires at least one scope",
        ));
    }

    let description = body.description.as_deref().unwrap_or("");

    db::roles::create_role(&state.pool, &body.name, description, false)
        .await
        .map_err(|e| {
            tracing::error!("failed to create role '{}': {e}", body.name);
            auth_error(
                StatusCode::CONFLICT,
                &format!("role '{}' already exists", body.name),
            )
        })?;

    // Set capabilities and scopes atomically
    let cap_refs: Vec<&str> = body.caps.iter().map(String::as_str).collect();
    let scope_entries: Vec<db::roles::ScopeEntry> =
        body.scopes.iter().cloned().map(Into::into).collect();
    db::roles::set_role_caps_and_scopes(&state.pool, &body.name, &cap_refs, &scope_entries)
        .await
        .map_err(|e| {
            tracing::error!("failed to set caps/scopes for role '{}': {e}", body.name);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set role capabilities and scopes",
            )
        })?;

    let role = db::roles::get_role(&state.pool, &body.name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get role '{}': {e}", body.name);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to get role")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "role not found after create",
            )
        })?;

    tracing::info!("user {} created role '{}'", user.email, body.name);

    Ok((
        StatusCode::CREATED,
        Json(RoleResponse {
            name: role.name,
            description: role.description,
            builtin: role.builtin,
            caps: body.caps,
            scopes: body.scopes,
            created_at: role.created_at,
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/permissions/roles/{name}
// ---------------------------------------------------------------------------

async fn get_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<RoleResponse>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:view")?;

    let role = db::roles::get_role(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to get role")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "role not found"))?;

    let caps = db::roles::get_role_caps(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get caps for role '{name}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get role capabilities",
            )
        })?;

    let scopes = db::roles::get_role_scopes(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get scopes for role '{name}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get role scopes",
            )
        })?;

    Ok(Json(RoleResponse {
        name: role.name,
        description: role.description,
        builtin: role.builtin,
        caps,
        scopes: scopes.into_iter().map(Into::into).collect(),
        created_at: role.created_at,
    }))
}

// ---------------------------------------------------------------------------
// PUT /api/permissions/roles/{name}
// ---------------------------------------------------------------------------

async fn update_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<CreateRoleBody>,
) -> Result<Json<RoleResponse>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;
    validate_caps(&body.caps)?;
    validate_scopes(&body.scopes)?;

    // Scope-dependent validation
    if role_is_scope_dependent(&body.caps) && body.scopes.is_empty() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "role contains scope-dependent capabilities and requires at least one scope",
        ));
    }

    // Check builtin
    if db::roles::is_role_builtin(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to check builtin for role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
    {
        return Err(auth_error(
            StatusCode::CONFLICT,
            "cannot modify a builtin role",
        ));
    }

    let role = db::roles::get_role(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to get role")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "role not found"))?;

    // Save old caps + scopes for possible rollback (last-admin guard)
    let old_caps = db::roles::get_role_caps(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get caps for role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;
    let old_scopes = db::roles::get_role_scopes(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get scopes for role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    let had_wildcard = old_caps.iter().any(|c| c == "*");
    let has_wildcard = body.caps.iter().any(|c| c == "*");

    // Set capabilities and scopes atomically
    let cap_refs: Vec<&str> = body.caps.iter().map(String::as_str).collect();
    let scope_entries: Vec<db::roles::ScopeEntry> =
        body.scopes.iter().cloned().map(Into::into).collect();
    db::roles::set_role_caps_and_scopes(&state.pool, &name, &cap_refs, &scope_entries)
        .await
        .map_err(|e| {
            tracing::error!("failed to update role '{name}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update role capabilities and scopes",
            )
        })?;

    // Last-admin guard: if we removed wildcard from a custom role
    if had_wildcard
        && !has_wildcard
        && let Err(e) = last_admin_guard(&state.pool).await
    {
        // Rollback: restore old caps and scopes
        let old_refs: Vec<&str> = old_caps.iter().map(String::as_str).collect();
        let _ =
            db::roles::set_role_caps_and_scopes(&state.pool, &name, &old_refs, &old_scopes).await;
        return Err(e);
    }

    tracing::info!("user {} updated role '{}'", user.email, name);

    Ok(Json(RoleResponse {
        name: role.name,
        description: role.description,
        builtin: role.builtin,
        caps: body.caps,
        scopes: body.scopes,
        created_at: role.created_at,
    }))
}

// ---------------------------------------------------------------------------
// DELETE /api/permissions/roles/{name}
// ---------------------------------------------------------------------------

async fn delete_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    // Check builtin
    if db::roles::is_role_builtin(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to check builtin for role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
    {
        return Err(auth_error(
            StatusCode::CONFLICT,
            "cannot delete a builtin role",
        ));
    }

    // If role has wildcard, check last-admin guard before deleting
    let caps = db::roles::get_role_caps(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get caps for role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    let has_wildcard = caps.iter().any(|c| c == "*");

    let deleted = db::roles::delete_role(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to delete role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete role")
        })?;

    if !deleted {
        return Err(auth_error(StatusCode::NOT_FOUND, "role not found"));
    }

    // Last-admin guard after deletion (cascade removes assignments + caps)
    if has_wildcard && let Err(e) = last_admin_guard(&state.pool).await {
        // Cannot easily rollback a DELETE CASCADE — the guard should prevent
        // the scenario entirely. Log and return the error.
        tracing::error!(
            "last-admin guard triggered after deleting role '{}' — this should not happen",
            name
        );
        return Err(e);
    }

    tracing::info!("user {} deleted role '{}'", user.email, name);
    Ok(Json(
        serde_json::json!({"detail": format!("role '{name}' deleted")}),
    ))
}
