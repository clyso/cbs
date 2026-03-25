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

use axum::extract::{Path, Query, State};
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
        // Roles
        .route("/roles", get(list_roles))
        .route("/roles", post(create_role))
        .route("/roles/{name}", get(get_role))
        .route("/roles/{name}", put(update_role_caps))
        .route("/roles/{name}", delete(delete_role))
        // Users
        .route("/users", get(list_users_with_roles))
        .route("/users/{email}/roles", get(get_user_roles))
        .route("/users/{email}/roles", put(replace_user_roles))
        .route("/users/{email}/roles", post(add_user_role))
        .route("/users/{email}/roles/{role}", delete(remove_user_role))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateRoleBody {
    name: String,
    description: Option<String>,
    caps: Vec<String>,
}

#[derive(Serialize)]
struct RoleResponse {
    name: String,
    description: String,
    builtin: bool,
    caps: Vec<String>,
    created_at: i64,
}

#[derive(Serialize)]
struct RoleListItem {
    name: String,
    description: String,
    builtin: bool,
    created_at: i64,
}

#[derive(Deserialize)]
struct DeleteRoleQuery {
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
struct ReplaceUserRolesBody {
    roles: Vec<RoleAssignmentBody>,
}

#[derive(Deserialize)]
struct AddUserRoleBody {
    role: String,
    #[serde(default)]
    scopes: Vec<ScopeBody>,
}

#[derive(Deserialize, Serialize, Clone)]
struct RoleAssignmentBody {
    role: String,
    #[serde(default)]
    scopes: Vec<ScopeBody>,
}

#[derive(Deserialize, Serialize, Clone)]
struct ScopeBody {
    #[serde(rename = "type")]
    scope_type: String,
    pattern: String,
}

#[derive(Serialize)]
struct UserWithRolesItem {
    email: String,
    name: String,
    active: bool,
    roles: Vec<UserRoleItem>,
}

#[derive(Serialize)]
struct UserRoleItem {
    role: String,
    scopes: Vec<ScopeBody>,
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

/// Validate scope entries. Channel scope patterns must contain `/` to
/// enforce the `channel/type` format (e.g. `ces/dev`, `ces/*`).
fn validate_scopes(scopes: &[ScopeBody]) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    for scope in scopes {
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

    // Set capabilities
    let cap_refs: Vec<&str> = body.caps.iter().map(String::as_str).collect();
    db::roles::set_role_caps(&state.pool, &body.name, &cap_refs)
        .await
        .map_err(|e| {
            tracing::error!("failed to set caps for role '{}': {e}", body.name);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set role capabilities",
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

    Ok(Json(RoleResponse {
        name: role.name,
        description: role.description,
        builtin: role.builtin,
        caps,
        created_at: role.created_at,
    }))
}

// ---------------------------------------------------------------------------
// PUT /api/permissions/roles/{name}
// ---------------------------------------------------------------------------

async fn update_role_caps(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
    Json(body): Json<CreateRoleBody>,
) -> Result<Json<RoleResponse>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;
    validate_caps(&body.caps)?;

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

    // If removing `*` from this role, run last-admin guard after the update
    let old_caps = db::roles::get_role_caps(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get caps for role '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    let had_wildcard = old_caps.iter().any(|c| c == "*");
    let has_wildcard = body.caps.iter().any(|c| c == "*");

    let cap_refs: Vec<&str> = body.caps.iter().map(String::as_str).collect();
    db::roles::set_role_caps(&state.pool, &name, &cap_refs)
        .await
        .map_err(|e| {
            tracing::error!("failed to set caps for role '{name}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set role capabilities",
            )
        })?;

    // Last-admin guard: if we removed wildcard from a custom role
    if had_wildcard
        && !has_wildcard
        && let Err(e) = last_admin_guard(&state.pool).await
    {
        // Rollback: restore old caps
        let old_refs: Vec<&str> = old_caps.iter().map(String::as_str).collect();
        let _ = db::roles::set_role_caps(&state.pool, &name, &old_refs).await;
        return Err(e);
    }

    tracing::info!("user {} updated caps for role '{}'", user.email, name);

    Ok(Json(RoleResponse {
        name: role.name,
        description: role.description,
        builtin: role.builtin,
        caps: body.caps,
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
    Query(params): Query<DeleteRoleQuery>,
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

    // Check for existing assignments unless force=true
    if !params.force {
        let has = db::roles::has_assignments(&state.pool, &name)
            .await
            .map_err(|e| {
                tracing::error!("failed to check assignments for role '{name}': {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        if has {
            return Err(auth_error(
                StatusCode::CONFLICT,
                "role has active assignments — use ?force=true to delete anyway",
            ));
        }
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

// ---------------------------------------------------------------------------
// GET /api/permissions/users
// ---------------------------------------------------------------------------

async fn list_users_with_roles(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<UserWithRolesItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:view")?;

    let users = sqlx::query!(
        r#"SELECT email AS "email!", name AS "name!", active AS "active!" FROM users ORDER BY email"#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("failed to list users: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list users")
    })?;

    let mut result = Vec::with_capacity(users.len());
    for row in users {
        let email = row.email;
        let name = row.name;
        let active: bool = row.active != 0;

        let user_roles = db::roles::get_user_roles(&state.pool, &email)
            .await
            .map_err(|e| {
                tracing::error!("failed to get roles for user '{email}': {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        let roles = user_roles
            .into_iter()
            .map(|ur| UserRoleItem {
                role: ur.role_name,
                scopes: ur
                    .scopes
                    .into_iter()
                    .map(|s| ScopeBody {
                        scope_type: s.scope_type,
                        pattern: s.pattern,
                    })
                    .collect(),
            })
            .collect();

        result.push(UserWithRolesItem {
            email,
            name,
            active,
            roles,
        });
    }

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// GET /api/permissions/users/{email}/roles
// ---------------------------------------------------------------------------

async fn get_user_roles(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
) -> Result<Json<Vec<UserRoleItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:view")?;

    let user_roles = db::roles::get_user_roles(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get roles for user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    Ok(Json(
        user_roles
            .into_iter()
            .map(|ur| UserRoleItem {
                role: ur.role_name,
                scopes: ur
                    .scopes
                    .into_iter()
                    .map(|s| ScopeBody {
                        scope_type: s.scope_type,
                        pattern: s.pattern,
                    })
                    .collect(),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/permissions/users/{email}/roles
// ---------------------------------------------------------------------------

async fn replace_user_roles(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
    Json(body): Json<ReplaceUserRolesBody>,
) -> Result<Json<Vec<UserRoleItem>>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    // Validate that all referenced roles exist
    for assignment in &body.roles {
        let role = db::roles::get_role(&state.pool, &assignment.role)
            .await
            .map_err(|e| {
                tracing::error!("failed to get role '{}': {e}", assignment.role);
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        if role.is_none() {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("role '{}' does not exist", assignment.role),
            ));
        }

        // Check scope-dependent roles have scopes
        let caps = db::roles::get_role_caps(&state.pool, &assignment.role)
            .await
            .map_err(|e| {
                tracing::error!("failed to get caps for role '{}': {e}", assignment.role);
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        if role_is_scope_dependent(&caps) && assignment.scopes.is_empty() {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "role '{}' contains scope-dependent capabilities and requires scopes",
                    assignment.role
                ),
            ));
        }

        // Validate channel scope patterns contain '/'.
        validate_scopes(&assignment.scopes)?;
    }

    // Convert to DB types
    let assignments: Vec<db::roles::RoleAssignment> = body
        .roles
        .into_iter()
        .map(|a| db::roles::RoleAssignment {
            role: a.role,
            scopes: a
                .scopes
                .into_iter()
                .map(|s| db::roles::ScopeEntry {
                    scope_type: s.scope_type,
                    pattern: s.pattern,
                })
                .collect(),
        })
        .collect();

    db::roles::set_user_roles(&state.pool, &email, &assignments)
        .await
        .map_err(|e| {
            tracing::error!("failed to set roles for user '{email}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set user roles",
            )
        })?;

    // Last-admin guard after replacement
    last_admin_guard(&state.pool).await?;

    tracing::info!("user {} replaced roles for user '{email}'", user.email);

    // Return the updated roles
    let user_roles = db::roles::get_user_roles(&state.pool, &email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get roles for user '{email}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    Ok(Json(
        user_roles
            .into_iter()
            .map(|ur| UserRoleItem {
                role: ur.role_name,
                scopes: ur
                    .scopes
                    .into_iter()
                    .map(|s| ScopeBody {
                        scope_type: s.scope_type,
                        pattern: s.pattern,
                    })
                    .collect(),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// POST /api/permissions/users/{email}/roles
// ---------------------------------------------------------------------------

async fn add_user_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path(email): Path<String>,
    Json(body): Json<AddUserRoleBody>,
) -> Result<(StatusCode, Json<UserRoleItem>), (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    // Validate role exists
    let role = db::roles::get_role(&state.pool, &body.role)
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

    // Check scope-dependent roles have scopes
    let caps = db::roles::get_role_caps(&state.pool, &role.name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get caps for role '{}': {e}", body.role);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if role_is_scope_dependent(&caps) && body.scopes.is_empty() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "role '{}' contains scope-dependent capabilities and requires scopes",
                body.role
            ),
        ));
    }

    // Validate channel scope patterns contain '/'.
    validate_scopes(&body.scopes)?;

    // Add the role assignment
    db::roles::add_user_role(&state.pool, &email, &body.role)
        .await
        .map_err(|e| {
            tracing::error!("failed to add role '{}' to user '{email}': {e}", body.role);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to add user role")
        })?;

    // Insert scopes for this assignment
    for scope in &body.scopes {
        sqlx::query!(
            "INSERT OR IGNORE INTO user_role_scopes (user_email, role_name, scope_type, pattern)
             VALUES (?, ?, ?, ?)",
            email,
            body.role,
            scope.scope_type,
            scope.pattern,
        )
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("failed to insert scope for assignment: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to add scope")
        })?;
    }

    tracing::info!(
        "user {} added role '{}' to user '{email}'",
        user.email,
        body.role
    );

    Ok((
        StatusCode::CREATED,
        Json(UserRoleItem {
            role: body.role,
            scopes: body
                .scopes
                .into_iter()
                .map(|s| ScopeBody {
                    scope_type: s.scope_type,
                    pattern: s.pattern,
                })
                .collect(),
        }),
    ))
}

// ---------------------------------------------------------------------------
// DELETE /api/permissions/users/{email}/roles/{role}
// ---------------------------------------------------------------------------

async fn remove_user_role(
    State(state): State<AppState>,
    user: AuthUser,
    Path((email, role)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    require_cap(&user, "permissions:manage")?;

    let removed = db::roles::remove_user_role(&state.pool, &email, &role)
        .await
        .map_err(|e| {
            tracing::error!("failed to remove role '{}' from user '{email}': {e}", role);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove user role",
            )
        })?;

    if !removed {
        return Err(auth_error(
            StatusCode::NOT_FOUND,
            "role assignment not found",
        ));
    }

    // Last-admin guard
    last_admin_guard(&state.pool).await?;

    tracing::info!(
        "user {} removed role '{}' from user '{email}'",
        user.email,
        role
    );

    Ok(Json(
        serde_json::json!({"detail": format!("role '{role}' removed from user '{email}'")}),
    ))
}
