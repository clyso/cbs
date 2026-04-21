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

//! Axum extractors for authentication and authorization.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::{Json, RequestPartsExt};
use axum_extra::TypedHeader;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use sqlx::SqlitePool;
use tower_sessions::Session;

use crate::app::AppState;
use crate::auth::paseto;
use crate::auth::token_cache;
use crate::db;

/// Scope types for per-assignment scope checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ScopeType {
    Channel,
    Registry,
    Repository,
}

impl ScopeType {
    /// Convert to the string stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Registry => "registry",
            Self::Repository => "repository",
        }
    }
}

impl std::fmt::Display for ScopeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Capabilities forbidden for robot accounts regardless of role assignments.
/// Robots cannot hold admin-level privileges or manage their own API keys.
/// Two layers enforce this: the auth-time strip in `load_authed_user`
/// (primary guard) and an assignment-time reject on the role-assign paths
/// (defense in depth — see `first_robot_forbidden_cap`).
pub(crate) const ROBOT_FORBIDDEN_CAPS: &[&str] = &[
    "*",
    "permissions:manage",
    "robots:manage",
    "apikeys:create:own",
];

/// Return the first cap in `caps` that robots are never allowed to hold, or
/// `None` if the set is safe to assign to a robot target. Used by the
/// assignment-time reject at the robot-create and entity role-assign paths.
pub(crate) fn first_robot_forbidden_cap(caps: &[String]) -> Option<&'static str> {
    caps.iter().find_map(|c| {
        ROBOT_FORBIDDEN_CAPS
            .iter()
            .copied()
            .find(|forb| *forb == c.as_str())
    })
}

/// Authenticated user extracted from `Authorization: Bearer` header or
/// session cookie (web UI fallback).
///
/// Bearer path distinguishes PASETO tokens, API keys (`cbsk_`), and robot
/// tokens (`cbrk_`) by prefix. Capabilities are loaded from the database
/// after user validation. Robot accounts have forbidden caps stripped after
/// the role-based cap set is computed.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub email: String,
    pub name: String,
    pub caps: Vec<String>,
    pub is_robot: bool,
}

impl AuthUser {
    /// Check if the user has a specific capability (or the wildcard `*`).
    pub fn has_cap(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == "*" || c == cap)
    }

    /// Check if the user has any of the given capabilities (OR).
    #[allow(dead_code)]
    pub fn has_any_cap(&self, caps: &[&str]) -> bool {
        caps.iter().any(|cap| self.has_cap(cap))
    }

    /// The display identity: for robots `name` (e.g. `robot:ci`), for
    /// humans `email`.
    pub fn display_identity(&self) -> &str {
        if self.is_robot {
            &self.name
        } else {
            &self.email
        }
    }

    /// Check that at least one of the user's assignments satisfies ALL
    /// scope checks. Loads assignments from the database.
    pub async fn require_scopes_all(
        &self,
        pool: &SqlitePool,
        scope_checks: &[(ScopeType, &str)],
    ) -> Result<(), AuthError> {
        let assignments = db::roles::get_user_assignments_with_scopes(pool, &self.email)
            .await
            .map_err(|_| {
                auth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load user assignments",
                )
            })?;

        // Find at least one assignment that satisfies ALL scope checks
        let ok = assignments.iter().any(|a| {
            scope_checks.iter().all(|(scope_type, value)| {
                if a.scopes.is_empty() {
                    return true;
                }
                a.scopes
                    .iter()
                    .filter(|s| s.scope_type == scope_type.as_str())
                    .any(|s| crate::scopes::scope_pattern_matches(&s.pattern, value))
            })
        });

        if ok {
            Ok(())
        } else {
            Err(auth_error(StatusCode::FORBIDDEN, "insufficient scopes"))
        }
    }
}

/// Error response body matching FastAPI's `{"detail": "..."}` shape.
#[derive(serde::Serialize)]
pub struct ErrorDetail {
    pub detail: String,
}

pub type AuthError = (StatusCode, Json<ErrorDetail>);

pub fn auth_error(status: StatusCode, msg: &str) -> AuthError {
    (
        status,
        Json(ErrorDetail {
            detail: msg.to_string(),
        }),
    )
}

/// Load an authenticated user from the database. Shared by all auth paths.
/// For robot accounts, forbidden caps are stripped from the effective cap set.
async fn load_authed_user(pool: &SqlitePool, email: &str) -> Result<AuthUser, AuthError> {
    let user = db::users::get_user(pool, email)
        .await
        .map_err(|_| auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to load user"))?
        .ok_or_else(|| auth_error(StatusCode::UNAUTHORIZED, "user not found"))?;

    if !user.active {
        return Err(auth_error(
            StatusCode::UNAUTHORIZED,
            "user account deactivated",
        ));
    }

    let mut caps = db::roles::get_effective_caps(pool, &user.email)
        .await
        .map_err(|_| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load capabilities",
            )
        })?;

    // Strip caps that robots are never allowed to hold, regardless of roles
    // (defense in depth — the assignment-time reject is the other layer).
    if user.is_robot {
        caps.retain(|c| !ROBOT_FORBIDDEN_CAPS.contains(&c.as_str()));
    }

    Ok(AuthUser {
        email: user.email,
        name: user.name,
        caps,
        is_robot: user.is_robot,
    })
}

/// Validate a raw PASETO token: decode, check revocation, load user.
/// Shared by the Bearer header and session cookie auth paths.
async fn validate_paseto(token_str: &str, state: &AppState) -> Result<AuthUser, AuthError> {
    let payload =
        paseto::token_decode(token_str, &state.config.secrets.token_secret_key).map_err(|e| {
            tracing::warn!(
                error = %e,
                token_prefix = &token_str[..token_str.len().min(20)],
                "auth reject: PASETO decode failed"
            );
            auth_error(StatusCode::UNAUTHORIZED, &format!("invalid token: {e}"))
        })?;

    let hash = paseto::token_hash(token_str);
    tracing::debug!(
        user = %payload.user,
        expires = ?payload.expires,
        hash_prefix = &hash[..16],
        "auth: PASETO decoded successfully"
    );

    let revoked = db::tokens::is_token_revoked(&state.pool, &hash)
        .await
        .map_err(|e| {
            tracing::warn!("auth reject: DB error checking revocation: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check token status",
            )
        })?;

    if revoked {
        tracing::warn!(
            user = %payload.user,
            hash_prefix = &hash[..16],
            "auth reject: token revoked or unknown"
        );
        return Err(auth_error(StatusCode::UNAUTHORIZED, "token revoked"));
    }

    tracing::debug!(user = %payload.user, "auth: token valid, loading user");
    let auth_user = load_authed_user(&state.pool, &payload.user).await?;

    // Usage tracking — never fail the request on a write error; the auth
    // path continues to succeed even if the update is lost.
    if let Err(e) = db::tokens::mark_token_used(&state.pool, &hash).await {
        tracing::warn!("failed to mark token used: {e}");
    }

    Ok(auth_user)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Path 1: Authorization: Bearer header (CLI, API keys, scripts)
        let bearer_result = parts.extract::<TypedHeader<Authorization<Bearer>>>().await;

        if let Ok(TypedHeader(Authorization(bearer))) = bearer_result {
            let token_str = bearer.token();
            tracing::debug!(
                token_prefix = &token_str[..token_str.len().min(20)],
                token_len = token_str.len(),
                "auth: processing bearer token"
            );

            // API key path
            if token_str.starts_with("cbsk_") {
                let cached =
                    token_cache::verify_api_key(&state.pool, &state.token_cache, token_str)
                        .await
                        .map_err(|e| {
                            tracing::warn!("auth reject: API key error: {e}");
                            auth_error(StatusCode::UNAUTHORIZED, &format!("API key error: {e}"))
                        })?;

                let auth_user = load_authed_user(&state.pool, &cached.owner_email).await?;

                // Usage tracking — warn-and-swallow on failure, inline await
                // so the request already holds the pool connection.
                if let Err(e) = db::api_keys::mark_api_key_used(&state.pool, cached.token_id).await
                {
                    tracing::warn!("failed to mark api key used: {e}");
                }

                return Ok(auth_user);
            }

            // Robot token path
            if token_str.starts_with("cbrk_") {
                let cached =
                    token_cache::verify_robot_token(&state.pool, &state.token_cache, token_str)
                        .await
                        .map_err(|e| {
                            tracing::warn!("auth reject: robot token error: {e}");
                            auth_error(StatusCode::UNAUTHORIZED, &format!("robot token error: {e}"))
                        })?;

                let auth_user = load_authed_user(&state.pool, &cached.owner_email).await?;

                // Usage tracking — warn-and-swallow on failure, inline await
                // so the request already holds the pool connection.
                if let Err(e) =
                    db::robots::mark_robot_token_used(&state.pool, cached.token_id).await
                {
                    tracing::warn!("failed to mark robot token used: {e}");
                }

                return Ok(auth_user);
            }

            // PASETO token path
            return validate_paseto(token_str, state).await;
        }

        // Path 2: Session cookie fallback (web UI)
        tracing::debug!("auth: no bearer header, trying session cookie");
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| auth_error(StatusCode::UNAUTHORIZED, "authentication required"))?;

        let raw_token: Option<String> = session.get("paseto_token").await.map_err(|e| {
            tracing::warn!("auth reject: session read failed: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
        })?;

        let Some(token_str) = raw_token else {
            tracing::debug!("auth reject: no bearer header and no session token");
            return Err(auth_error(
                StatusCode::UNAUTHORIZED,
                "authentication required",
            ));
        };

        tracing::debug!("auth: processing token from session cookie");
        validate_paseto(&token_str, state).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_robot_forbidden_cap_rejects_each_forbidden_cap() {
        for &cap in ROBOT_FORBIDDEN_CAPS {
            let caps = vec!["builds:create".to_string(), cap.to_string()];
            let found = first_robot_forbidden_cap(&caps);
            assert_eq!(found, Some(cap), "expected to flag '{cap}'");
        }
    }

    #[test]
    fn first_robot_forbidden_cap_allows_safe_set() {
        let caps = vec![
            "builds:create".to_string(),
            "builds:list:own".to_string(),
            "channels:view".to_string(),
        ];
        assert_eq!(first_robot_forbidden_cap(&caps), None);
    }

    #[test]
    fn display_identity_returns_name_for_robot_and_email_for_human() {
        let robot = AuthUser {
            email: "robot+ci@robots".to_string(),
            name: "robot:ci".to_string(),
            caps: Vec::new(),
            is_robot: true,
        };
        assert_eq!(robot.display_identity(), "robot:ci");

        let human = AuthUser {
            email: "alice@example.com".to_string(),
            name: "Alice".to_string(),
            caps: Vec::new(),
            is_robot: false,
        };
        assert_eq!(human.display_identity(), "alice@example.com");
    }
}
