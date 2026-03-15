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

use crate::app::AppState;
use crate::auth::paseto;
use crate::db;

/// Authenticated user extracted from the `Authorization: Bearer` header.
///
/// Distinguishes PASETO tokens from API keys by the `cbsk_` prefix.
/// For Commit 3, only PASETO tokens are supported. API key support is
/// added in Commit 4.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub email: String,
    pub name: String,
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

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Extract Bearer token
        let TypedHeader(Authorization(bearer)) = parts
            .extract::<TypedHeader<Authorization<Bearer>>>()
            .await
            .map_err(|_| {
                auth_error(
                    StatusCode::UNAUTHORIZED,
                    "missing or invalid Authorization header",
                )
            })?;

        let token_str = bearer.token();

        // Distinguish API keys (cbsk_ prefix) from PASETO tokens
        if token_str.starts_with("cbsk_") {
            // API key authentication — added in Commit 4
            return Err(auth_error(
                StatusCode::UNAUTHORIZED,
                "API key authentication not yet implemented",
            ));
        }

        // PASETO token path
        let payload = paseto::token_decode(token_str, &state.config.secrets.token_secret_key)
            .map_err(|e| auth_error(StatusCode::UNAUTHORIZED, &format!("invalid token: {e}")))?;

        // Check revocation in DB
        let hash = paseto::token_hash(token_str);
        let revoked = db::tokens::is_token_revoked(&state.pool, &hash)
            .await
            .map_err(|_| {
                auth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to check token status",
                )
            })?;

        if revoked {
            return Err(auth_error(StatusCode::UNAUTHORIZED, "token revoked"));
        }

        // Load user record and check active status in a single query
        let user = db::users::get_user(&state.pool, &payload.user)
            .await
            .map_err(|_| {
                auth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load user",
                )
            })?
            .ok_or_else(|| auth_error(StatusCode::UNAUTHORIZED, "user not found"))?;

        if !user.active {
            return Err(auth_error(
                StatusCode::UNAUTHORIZED,
                "user account deactivated",
            ));
        }

        Ok(AuthUser {
            email: user.email,
            name: user.name,
        })
    }
}
