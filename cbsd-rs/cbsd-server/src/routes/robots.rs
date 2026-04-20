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

//! Route handlers for `/api/admin/robots/*`: robot account lifecycle.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::auth::extractors::{
    AuthUser, ErrorDetail, ROBOT_FORBIDDEN_CAPS, auth_error, first_robot_forbidden_cap,
};
use crate::auth::token_cache;
use crate::db;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_or_revive_robot))
        .route("/", get(list_robots))
        .route("/{name}", get(get_robot))
        .route("/{name}", delete(tombstone_robot))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateRobotBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Required per design v4. Accepts a `"YYYY-MM-DD"` string (UTC date,
    /// token valid through the end of that day) or `null` (no expiry).
    /// The field must be present; an omitted `expires` returns 400 so the
    /// "never-expiring token" path stays an explicit caller opt-in.
    expires: serde_json::Value,
    #[serde(default)]
    roles: Vec<String>,
}

#[derive(Serialize)]
struct CreateRobotResponse {
    name: String,
    display_name: String,
    email: String,
    active: bool,
    description: Option<String>,
    token: String,
    token_prefix: String,
    token_expires_at: Option<i64>,
    created_at: i64,
    roles: Vec<String>,
    revived: bool,
}

/// List item for `GET /api/admin/robots` — design v4 § REST API "List
/// Robots" shape.
#[derive(Serialize)]
struct RobotListItem {
    name: String,
    display_name: String,
    email: String,
    description: Option<String>,
    active: bool,
    created_at: i64,
    /// `"active" | "expired" | "revoked" | "none"`.
    token_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<i64>,
}

/// Detail view for `GET /api/admin/robots/{name}` — design v4 § REST API
/// "Get Robot" shape.
#[derive(Serialize)]
struct RobotDetail {
    name: String,
    display_name: String,
    email: String,
    description: Option<String>,
    active: bool,
    created_at: i64,
    token_status: TokenStatusBody,
    roles: Vec<String>,
    effective_caps: Vec<String>,
}

#[derive(Serialize)]
struct TokenStatusBody {
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_used_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_created_at: Option<i64>,
}

impl From<&db::robots::RobotRow> for TokenStatusBody {
    fn from(r: &db::robots::RobotRow) -> Self {
        Self {
            state: r.token_state.clone(),
            prefix: r.token_prefix.clone(),
            expires_at: r.token_expires_at,
            first_used_at: r.token_first_used_at,
            last_used_at: r.token_last_used_at,
            token_created_at: r.token_created_at,
        }
    }
}

/// Parse the `expires` wire value (design v4). Accepts an ISO calendar
/// date string `"YYYY-MM-DD"` or JSON `null`. Returns `None` for `null`
/// (meaning "never expires") or `Some(epoch)` where epoch is the start of
/// the UTC day **after** the given date, so the token is valid through
/// the end of the named day. Unknown JSON shapes or malformed dates
/// return a 400 message.
fn parse_expires_wire(v: &serde_json::Value) -> Result<Option<i64>, String> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => parse_iso_date_to_next_day_epoch(s).map(Some),
        _ => Err("expires must be a YYYY-MM-DD string or null".to_string()),
    }
}

fn parse_iso_date_to_next_day_epoch(s: &str) -> Result<i64, String> {
    let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("invalid expires date '{s}': {e}"))?;
    let next_day = date
        .succ_opt()
        .ok_or_else(|| format!("date '{s}' overflows when computing the next day"))?;
    let dt = next_day
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| "time overflow computing epoch".to_string())?
        .and_utc();
    Ok(dt.timestamp())
}

// ---------------------------------------------------------------------------
// POST /api/admin/robots
// ---------------------------------------------------------------------------

/// Create a new robot account, or revive a tombstoned one with a fresh token.
///
/// If a robot with the given name already exists and is active, returns 409.
/// If it exists but is tombstoned (active=0), revives it: re-activates,
/// replaces roles, revokes old tokens, issues a new token.
async fn create_or_revive_robot(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateRobotBody>,
) -> Result<(StatusCode, Json<CreateRobotResponse>), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("robots:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: robots:manage",
        ));
    }

    db::robots::validate_robot_name(&body.name)
        .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &format!("invalid robot name: {e}")))?;

    // Validate roles exist and do not carry caps forbidden for robot targets.
    // The auth-time strip in load_authed_user is the primary guard; this
    // assignment-time check is defense in depth and keeps operators from ever
    // seeing a robot visibly "holding" an admin cap on its role list.
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

        let caps = db::roles::get_role_caps(&state.pool, role_name)
            .await
            .map_err(|e| {
                tracing::error!("failed to load caps for role '{role_name}': {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;

        if let Some(forbidden) = first_robot_forbidden_cap(&caps) {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                &format!("role '{role_name}' carries cap '{forbidden}' which robots cannot hold"),
            ));
        }
    }

    // Parse the `expires` wire value (string date → day-after epoch, or null
    // for no expiry). Absent field is rejected at JSON-deserialise time.
    let expires_at = parse_expires_wire(&body.expires)
        .map_err(|msg| auth_error(StatusCode::BAD_REQUEST, &msg))?;

    let email = db::robots::name_to_synthetic_email(&body.name);

    // Generate token material before opening the transaction (Argon2 is CPU-bound)
    let (plaintext_token, token_prefix, token_hash) = token_cache::generate_robot_token_material()
        .await
        .map_err(|e| {
            tracing::error!("failed to generate robot token material: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate robot token",
            )
        })?;

    let role_refs: Vec<&str> = body.roles.iter().map(String::as_str).collect();
    let desc = body.description.as_deref();

    let outcome = db::robots::create_or_revive(
        &state.pool,
        &body.name,
        desc,
        expires_at,
        &token_hash,
        &token_prefix,
        &role_refs,
    )
    .await
    .map_err(|e| match e {
        db::robots::CreateRobotError::AlreadyActive => auth_error(
            StatusCode::CONFLICT,
            &format!("robot '{}' already exists and is active", body.name),
        ),
        db::robots::CreateRobotError::HumanCollision => auth_error(
            StatusCode::CONFLICT,
            &format!(
                "name '{}' conflicts with an existing human account",
                body.name
            ),
        ),
        db::robots::CreateRobotError::UniqueViolation => auth_error(
            StatusCode::CONFLICT,
            &format!(
                "robot '{}' was modified concurrently; retry the request",
                body.name
            ),
        ),
        db::robots::CreateRobotError::Db(err) => {
            tracing::error!("failed to create/revive robot '{}': {err}", body.name);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create robot")
        }
    })?;

    let revived = matches!(outcome, db::robots::CreateRevivedOutcome::Revived);

    // On revive, purge any stale cache entries for the prior identity.
    if revived {
        let mut cache = state.token_cache.lock().await;
        cache.remove_by_owner(&email);
    }

    // Refetch the row so the response's `display_name`, `active`, and
    // `created_at` reflect the committed state (in particular, revive
    // resets created_at — see F3).
    let committed = db::robots::get_robot_by_name(&state.pool, &body.name)
        .await
        .map_err(|e| {
            tracing::error!("failed to refetch robot '{}': {e}", body.name);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "robot disappeared after create",
            )
        })?;

    tracing::info!(
        "user {} {} robot '{}' (prefix={})",
        user.display_identity(),
        if revived { "revived" } else { "created" },
        body.name,
        token_prefix,
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateRobotResponse {
            name: body.name,
            display_name: committed.display_name,
            email: committed.email,
            active: committed.active,
            description: committed.description,
            token: plaintext_token,
            token_prefix,
            token_expires_at: expires_at,
            created_at: committed.created_at,
            roles: body.roles,
            revived,
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/admin/robots
// ---------------------------------------------------------------------------

async fn list_robots(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<RobotListItem>>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("robots:view") && !user.has_cap("robots:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: robots:view",
        ));
    }

    let robots = db::robots::list_robots(&state.pool).await.map_err(|e| {
        tracing::error!("failed to list robots: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list robots")
    })?;

    Ok(Json(
        robots
            .into_iter()
            .map(|r| {
                let name = db::robots::synthetic_email_to_name(&r.email)
                    .unwrap_or(&r.email)
                    .to_string();
                RobotListItem {
                    name,
                    display_name: r.display_name,
                    email: r.email,
                    description: r.description,
                    active: r.active,
                    created_at: r.created_at,
                    token_state: r.token_state,
                    token_expires_at: r.token_expires_at,
                    last_used_at: r.token_last_used_at,
                }
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/admin/robots/{name}
// ---------------------------------------------------------------------------

async fn get_robot(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<RobotDetail>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("robots:view") && !user.has_cap("robots:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: robots:view",
        ));
    }

    let robot = db::robots::get_robot_by_name(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get robot '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "robot not found"))?;

    let role_entries = db::roles::get_user_roles(&state.pool, &robot.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to load roles for robot '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;
    let roles: Vec<String> = role_entries.into_iter().map(|r| r.role_name).collect();

    // Effective caps with forbidden-cap strip — mirrors load_authed_user so
    // the visible cap set in the API response is exactly what the auth path
    // would give the robot at decision time.
    let mut effective_caps = db::roles::get_effective_caps(&state.pool, &robot.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to load caps for robot '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;
    effective_caps.retain(|c| !ROBOT_FORBIDDEN_CAPS.contains(&c.as_str()));
    effective_caps.sort();

    let token_status = TokenStatusBody::from(&robot);

    Ok(Json(RobotDetail {
        name,
        display_name: robot.display_name,
        email: robot.email,
        description: robot.description,
        active: robot.active,
        created_at: robot.created_at,
        token_status,
        roles,
        effective_caps,
    }))
}

// ---------------------------------------------------------------------------
// DELETE /api/admin/robots/{name}
// ---------------------------------------------------------------------------

/// Tombstone a robot: set active=0, revoke all tokens, purge cache.
/// Idempotent — deleting an already-tombstoned robot returns 200.
///
/// The name-to-email lookup runs outside the DB helper's
/// `BEGIN IMMEDIATE` transaction. This is deliberate and safe:
/// - The 404 branch performs no write, so no transaction is needed.
/// - If a concurrent revive reactivates the robot between the read and
///   the tombstone's own `BEGIN IMMEDIATE`, the tombstone still succeeds
///   idempotently (active → 0, any non-revoked token → revoked). The
///   revive-racer observes its work undone; the final state is
///   self-consistent and matches the caller's intent.
async fn tombstone_robot(
    State(state): State<AppState>,
    user: AuthUser,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("robots:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: robots:manage",
        ));
    }

    let robot = db::robots::get_robot_by_name(&state.pool, &name)
        .await
        .map_err(|e| {
            tracing::error!("failed to get robot '{name}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "robot not found"))?;

    let email = &robot.email;

    let tokens_revoked = db::robots::tombstone_robot(&state.pool, email)
        .await
        .map_err(|e| {
            tracing::error!("failed to tombstone robot '{name}': {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to tombstone robot",
            )
        })?;

    // Purge all cached tokens for this robot.
    {
        let mut cache = state.token_cache.lock().await;
        cache.remove_by_owner(email);
    }

    tracing::info!(
        "user {} tombstoned robot '{}' (revoked {tokens_revoked} tokens)",
        user.display_identity(),
        name,
    );

    Ok(Json(serde_json::json!({
        "detail": format!("robot '{name}' tombstoned"),
        "tokens_revoked": tokens_revoked,
    })))
}

#[cfg(test)]
mod handler_tests {
    use super::*;
    use crate::routes::test_support::{auth_user, test_app_state, test_pool};
    use axum::extract::{Path, State};

    #[tokio::test]
    async fn get_robot_of_unknown_name_returns_404() {
        let pool = test_pool().await;
        let state = test_app_state(pool);
        let caller = auth_user("alice@example.com", "Alice", false, &["robots:view"]);

        match get_robot(State(state), caller, Path("ghost".to_string())).await {
            Err((status, _)) => assert_eq!(status, StatusCode::NOT_FOUND),
            Ok(_) => panic!("unknown robot name must 404"),
        }
    }

    #[tokio::test]
    async fn tombstone_robot_of_unknown_name_returns_404() {
        let pool = test_pool().await;
        let state = test_app_state(pool);
        let caller = auth_user("alice@example.com", "Alice", false, &["robots:manage"]);

        match tombstone_robot(State(state), caller, Path("ghost".to_string())).await {
            Err((status, _)) => assert_eq!(status, StatusCode::NOT_FOUND),
            Ok(_) => panic!("unknown robot name must 404"),
        }
    }

    #[tokio::test]
    async fn get_robot_without_view_cap_returns_403() {
        let pool = test_pool().await;
        let state = test_app_state(pool);
        let caller = auth_user("alice@example.com", "Alice", false, &[]);

        match get_robot(State(state), caller, Path("ghost".to_string())).await {
            Err((status, _)) => assert_eq!(status, StatusCode::FORBIDDEN),
            Ok(_) => panic!("missing robots:view must 403"),
        }
    }

    #[tokio::test]
    async fn tombstone_robot_without_manage_cap_returns_403() {
        let pool = test_pool().await;
        let state = test_app_state(pool);
        let caller = auth_user("alice@example.com", "Alice", false, &[]);

        match tombstone_robot(State(state), caller, Path("ghost".to_string())).await {
            Err((status, _)) => assert_eq!(status, StatusCode::FORBIDDEN),
            Ok(_) => panic!("missing robots:manage must 403"),
        }
    }
}

#[cfg(test)]
mod expires_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn null_means_no_expiry() {
        assert_eq!(parse_expires_wire(&serde_json::Value::Null).unwrap(), None);
    }

    #[test]
    fn date_string_yields_next_day_midnight_utc_epoch() {
        // 2026-12-31 → token valid through 2026-12-31 UTC; stored as epoch
        // of 2027-01-01 00:00:00 UTC = 1798761600.
        let v = json!("2026-12-31");
        let epoch = parse_expires_wire(&v).unwrap().unwrap();
        assert_eq!(epoch, 1798761600);

        // Round-trip: 2027-01-01 → 2027-01-02 00:00:00 UTC.
        let v = json!("2027-01-01");
        let epoch = parse_expires_wire(&v).unwrap().unwrap();
        assert_eq!(epoch, 1798848000);
    }

    #[test]
    fn invalid_date_string_is_rejected() {
        let v = json!("not-a-date");
        assert!(parse_expires_wire(&v).is_err());
    }

    #[test]
    fn non_string_non_null_is_rejected() {
        assert!(parse_expires_wire(&json!(12345)).is_err());
        assert!(parse_expires_wire(&json!(true)).is_err());
        assert!(parse_expires_wire(&json!([])).is_err());
        assert!(parse_expires_wire(&json!({})).is_err());
    }
}
