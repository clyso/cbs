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

//! Auth route handlers: OAuth login/callback, token management, API keys.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tower_governor::GovernorLayer;
use tower_governor::errors::GovernorError;
use tower_governor::governor::GovernorConfigBuilder;
use tower_sessions::Session;

use crate::app::AppState;
use crate::auth::api_keys;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error};
use crate::auth::oauth;
use crate::auth::paseto;
use crate::db;

/// Build the auth sub-router: `/api/auth/*`.
///
/// Rate limiting: `/login` and `/callback` are limited to 10 req/min per IP
/// via `tower-governor`. Authenticated endpoints are not rate-limited here.
pub fn router() -> Router<AppState> {
    // Rate limit: 10 requests per 60 seconds per IP
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(60)
            .burst_size(10)
            .error_handler(|err| {
                tracing::error!("rate limiter: {err}");
                // Return JSON error matching the API's ErrorDetail
                // format instead of tower_governor's plain-text default.
                let (status, message) = match &err {
                    GovernorError::TooManyRequests { .. } => {
                        (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded")
                    }
                    GovernorError::UnableToExtractKey => {
                        (StatusCode::INTERNAL_SERVER_ERROR, "unable to extract client IP")
                    }
                    GovernorError::Other { .. } => {
                        (StatusCode::INTERNAL_SERVER_ERROR, "rate limiter error")
                    }
                };
                let body = serde_json::json!({"error": message});
                axum::http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap()
            })
            .finish()
            .expect("failed to build governor config"),
    );
    let governor_layer = GovernorLayer {
        config: governor_conf,
    };

    // Rate-limited OAuth routes
    let oauth_routes = Router::new()
        .route("/login", get(login))
        .route("/callback", get(callback))
        .layer(governor_layer);

    // Non-rate-limited authenticated routes
    let auth_routes = Router::new()
        .route("/whoami", get(whoami))
        .route("/token/revoke", post(revoke_token))
        .route("/tokens/revoke-all", post(revoke_all_tokens))
        .route("/api-keys", post(create_api_key_handler))
        .route("/api-keys", get(list_api_keys_handler))
        .route("/api-keys/{prefix}", delete(revoke_api_key_handler));

    oauth_routes.merge(auth_routes)
}

// ---------------------------------------------------------------------------
// Query / body types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginQuery {
    /// Client type: "cli" or "web".
    #[serde(default = "default_client")]
    client: String,
    /// Optional port for CLI localhost callback redirect.
    cli_port: Option<u16>,
}

fn default_client() -> String {
    "cli".to_string()
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: String,
    dev_email: Option<String>,
}

#[derive(Serialize)]
struct WhoamiResponse {
    email: String,
    name: String,
    roles: Vec<String>,
    effective_caps: Vec<String>,
}

#[derive(Deserialize)]
struct RevokeAllBody {
    user_email: String,
}

#[derive(Deserialize)]
struct CreateApiKeyBody {
    name: String,
}

#[derive(Serialize)]
struct CreateApiKeyResponse {
    key: String,
    prefix: String,
    name: String,
}

#[derive(Serialize)]
struct ApiKeyItem {
    prefix: String,
    name: String,
    created_at: i64,
}

// ---------------------------------------------------------------------------
// GET /api/auth/login
// ---------------------------------------------------------------------------

async fn login(
    State(state): State<AppState>,
    Query(params): Query<LoginQuery>,
    session: Session,
) -> Result<Response, (StatusCode, Json<ErrorDetail>)> {
    let oauth_nonce = uuid::Uuid::new_v4().to_string();

    // Store state + client info in session
    session
        .insert("oauth_state", &oauth_nonce)
        .await
        .map_err(|e| {
            tracing::error!("session insert failed: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
        })?;
    session
        .insert("client_type", &params.client)
        .await
        .map_err(|e| {
            tracing::error!("session insert failed: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
        })?;
    if let Some(port) = params.cli_port {
        session.insert("cli_port", port).await.map_err(|e| {
            tracing::error!("session insert failed: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
        })?;
    }

    // Dev mode: skip Google, redirect to callback with seed_admin email.
    if state.config.dev.enabled {
        let email = state.config.seed.seed_admin.as_ref().ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "dev mode requires seed-admin in config",
            )
        })?;
        let url = format!(
            "/api/auth/callback?state={oauth_nonce}&dev_email={email}"
        );
        return Ok(Redirect::temporary(&url).into_response());
    }

    // Use first allowed domain as hd hint if there is exactly one
    let hd = if state.config.oauth.allowed_domains.len() == 1 {
        Some(state.config.oauth.allowed_domains[0].as_str())
    } else {
        None
    };

    let url = oauth::build_google_auth_url(&state.oauth, &oauth_nonce, hd);
    Ok(Redirect::temporary(&url).into_response())
}

// ---------------------------------------------------------------------------
// GET /api/auth/callback
// ---------------------------------------------------------------------------

async fn callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackQuery>,
    session: Session,
) -> Result<Response, (StatusCode, Json<ErrorDetail>)> {
    // Validate OAuth state nonce from session
    let stored_state: Option<String> = session.get("oauth_state").await.map_err(|e| {
        tracing::error!("session get failed: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
    })?;

    let Some(stored_state) = stored_state else {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "missing OAuth state in session",
        ));
    };

    if stored_state != params.state {
        return Err(auth_error(StatusCode::BAD_REQUEST, "OAuth state mismatch"));
    }

    // Read client_type and cli_port before cycling the session
    let client_type: String = session
        .get("client_type")
        .await
        .map_err(|e| {
            tracing::error!("session get failed: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
        })?
        .unwrap_or_else(|| "web".to_string());

    let cli_port: Option<u16> = session.get("cli_port").await.map_err(|e| {
        tracing::error!("session get failed: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
    })?;

    // Resolve user info — dev mode or Google exchange.
    let user_info = if state.config.dev.enabled && params.dev_email.is_some() {
        let email = params.dev_email.unwrap();
        let name = email.split('@').next().unwrap_or(&email).to_string();
        oauth::GoogleUserInfo { email, name }
    } else {
        let code = params.code.ok_or_else(|| {
            auth_error(StatusCode::BAD_REQUEST, "missing authorization code")
        })?;

        let info = oauth::exchange_code_for_userinfo(&state.oauth, &code)
            .await
            .map_err(|e| {
                tracing::error!("OAuth code exchange failed: {e}");
                auth_error(
                    StatusCode::BAD_GATEWAY,
                    "failed to authenticate with Google",
                )
            })?;

        // Check domain restriction (production only).
        if !state.config.oauth.allow_any_google_account {
            let domain = info
                .email
                .rsplit_once('@')
                .map(|(_, d)| d)
                .unwrap_or("");

            if !state
                .config
                .oauth
                .allowed_domains
                .iter()
                .any(|d| d == domain)
            {
                return Err(auth_error(
                    StatusCode::FORBIDDEN,
                    "email domain not allowed",
                ));
            }
        }

        info
    };

    // Create or update user in DB
    let _user = db::users::create_or_update_user(&state.pool, &user_info.email, &user_info.name)
        .await
        .map_err(|e| {
            tracing::error!("failed to create/update user: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create user")
        })?;

    // Create PASETO token (with max_ttl clamping)
    // Default: 24h expiry
    let default_ttl: i64 = 86400;
    let expires_at = Some(chrono::Utc::now().timestamp() + default_ttl);

    let (raw_token, token_hash) = paseto::token_create(
        &user_info.email,
        expires_at,
        state.config.secrets.max_token_ttl_seconds,
        &state.config.secrets.token_secret_key,
    )
    .map_err(|e| {
        tracing::error!("token creation failed: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create token")
    })?;

    // Insert token hash into DB
    db::tokens::insert_token(&state.pool, &user_info.email, &token_hash, expires_at)
        .await
        .map_err(|e| {
            tracing::error!("failed to store token: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to store token")
        })?;

    // Regenerate session ID (cycle to prevent session fixation)
    session.cycle_id().await.map_err(|e| {
        tracing::error!("session cycle failed: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
    })?;

    // Encode token as base64 for transport
    let token_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_token.as_bytes());

    // Respond based on client type
    if client_type == "cli" {
        if let Some(port) = cli_port {
            // CLI with port: redirect to localhost
            let html = format!(
                r#"<!DOCTYPE html>
<html><head><title>Authentication Successful</title></head>
<body>
<p>Authentication successful. Redirecting...</p>
<script>window.location.href = "http://localhost:{port}/callback?token={token_b64}";</script>
</body></html>"#
            );
            Ok(Html(html).into_response())
        } else {
            // CLI without port: display token with strict CSP
            let html = format!(
                r#"<!DOCTYPE html>
<html><head><title>Authentication Successful</title></head>
<body>
<h1>Authentication Successful</h1>
<p>Copy this token to your CLI:</p>
<pre id="token">{token_b64}</pre>
</body></html>"#
            );
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-security-policy",
                HeaderValue::from_static("default-src 'none'; script-src 'none'"),
            );
            Ok((headers, Html(html)).into_response())
        }
    } else {
        // Web client: redirect with token fragment
        let redirect_url = format!("/#token={token_b64}");
        Ok(Redirect::temporary(&redirect_url).into_response())
    }
}

// ---------------------------------------------------------------------------
// GET /api/auth/whoami
// ---------------------------------------------------------------------------

async fn whoami(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<WhoamiResponse>, (StatusCode, Json<ErrorDetail>)> {
    let user_roles = db::roles::get_user_roles(&state.pool, &user.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get roles for whoami: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load user roles",
            )
        })?;

    let roles: Vec<String> = user_roles.into_iter().map(|ur| ur.role_name).collect();

    Ok(Json(WhoamiResponse {
        effective_caps: user.caps.clone(),
        email: user.email,
        name: user.name,
        roles,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/auth/token/revoke
// ---------------------------------------------------------------------------

/// Self-revoke: revokes the bearer token used in the current request.
async fn revoke_token(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Re-extract the raw bearer token from Authorization header
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| auth_error(StatusCode::UNAUTHORIZED, "missing bearer token"))?;

    // Only PASETO tokens can be self-revoked via this endpoint
    if auth_header.starts_with("cbsk_") {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "use DELETE /api/auth/api-keys/:prefix to revoke API keys",
        ));
    }

    let hash = paseto::token_hash(auth_header);
    db::tokens::revoke_token(&state.pool, &hash)
        .await
        .map_err(|e| {
            tracing::error!("failed to revoke token for {}: {e}", user.email);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke token")
        })?;

    tracing::info!("user {} revoked their token", user.email);
    Ok(Json(serde_json::json!({"detail": "token revoked"})))
}

// ---------------------------------------------------------------------------
// POST /api/auth/tokens/revoke-all
// ---------------------------------------------------------------------------

/// Revoke all tokens for a user. Requires the target user to exist.
/// Full permission check (permissions:manage) is added in Commit 5.
async fn revoke_all_tokens(
    State(state): State<AppState>,
    _user: AuthUser,
    Json(body): Json<RevokeAllBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Verify target user exists
    let target = db::users::get_user(&state.pool, &body.user_email)
        .await
        .map_err(|e| {
            tracing::error!("failed to look up user: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if target.is_none() {
        return Err(auth_error(StatusCode::NOT_FOUND, "user not found"));
    }

    let count = db::tokens::revoke_all_for_user(&state.pool, &body.user_email)
        .await
        .map_err(|e| {
            tracing::error!("failed to revoke tokens: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke tokens")
        })?;

    tracing::info!("revoked {count} tokens for user {}", body.user_email);
    Ok(Json(
        serde_json::json!({"detail": format!("revoked {count} tokens")}),
    ))
}

// ---------------------------------------------------------------------------
// POST /api/auth/api-keys
// ---------------------------------------------------------------------------

async fn create_api_key_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), (StatusCode, Json<ErrorDetail>)> {
    // Reject reserved prefix — worker keys are managed via /api/admin/workers
    if body.name.starts_with("worker:") {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "key names starting with 'worker:' are reserved for worker registration",
        ));
    }

    let (key, prefix) = api_keys::create_api_key(&state.pool, &body.name, &user.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to create API key for {}: {e}", user.email);
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create API key",
            )
        })?;

    tracing::info!(
        "created API key '{}' (prefix={}) for {}",
        body.name,
        prefix,
        user.email
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            key,
            prefix,
            name: body.name,
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/auth/api-keys
// ---------------------------------------------------------------------------

async fn list_api_keys_handler(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ApiKeyItem>>, (StatusCode, Json<ErrorDetail>)> {
    let keys = db::api_keys::list_api_keys_for_user(&state.pool, &user.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to list API keys for {}: {e}", user.email);
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list API keys")
        })?;

    Ok(Json(
        keys.into_iter()
            // Filter out worker-bound keys — managed via /api/admin/workers
            .filter(|k| !k.name.starts_with("worker:"))
            .map(|k| ApiKeyItem {
                prefix: k.key_prefix,
                name: k.name,
                created_at: k.created_at,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// DELETE /api/auth/api-keys/:prefix
// ---------------------------------------------------------------------------

async fn revoke_api_key_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(prefix): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    let revoked = db::api_keys::revoke_api_key_by_prefix(&state.pool, &user.email, &prefix)
        .await
        .map_err(|e| {
            tracing::error!("failed to revoke API key: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to revoke API key",
            )
        })?;

    if !revoked {
        return Err(auth_error(StatusCode::NOT_FOUND, "API key not found"));
    }

    // Purge from cache
    let mut cache = state.api_key_cache.lock().await;
    cache.remove_by_prefix(&prefix);

    tracing::info!("revoked API key (prefix={}) for {}", prefix, user.email);
    Ok(Json(serde_json::json!({"detail": "API key revoked"})))
}
