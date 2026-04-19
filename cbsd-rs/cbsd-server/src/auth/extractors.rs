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

/// Authenticated user extracted from `Authorization: Bearer` header or
/// session cookie (web UI fallback).
///
/// Bearer path distinguishes PASETO tokens from API keys by the `cbsk_`
/// prefix. Session path reads the PASETO token stored server-side by
/// the OAuth callback. Capabilities are loaded from the database after
/// user validation.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub email: String,
    pub name: String,
    pub caps: Vec<String>,
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

/// Load an authenticated user from the database. Shared by both PASETO and
/// API key auth paths to avoid logic duplication.
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

    let caps = db::roles::get_effective_caps(pool, &user.email)
        .await
        .map_err(|_| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load capabilities",
            )
        })?;

    Ok(AuthUser {
        email: user.email,
        name: user.name,
        caps,
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
                let cached = crate::auth::api_keys::verify_api_key(
                    &state.pool,
                    &state.api_key_cache,
                    token_str,
                )
                .await
                .map_err(|e| {
                    tracing::warn!("auth reject: API key error: {e}");
                    auth_error(StatusCode::UNAUTHORIZED, &format!("API key error: {e}"))
                })?;

                let auth_user = load_authed_user(&state.pool, &cached.owner_email).await?;

                // Usage tracking — warn-and-swallow on failure, inline await
                // so the request already holds the pool connection.
                if let Err(e) =
                    db::api_keys::mark_api_key_used(&state.pool, cached.api_key_id).await
                {
                    tracing::warn!("failed to mark api key used: {e}");
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
