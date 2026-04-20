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

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tower_governor::GovernorLayer;
use tower_governor::errors::GovernorError;
use tower_governor::governor::GovernorConfigBuilder;
use tower_sessions::Session;

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, auth_error};
use crate::auth::oauth;
use crate::auth::paseto;
use crate::auth::token_cache;
use crate::config::WEB_SESSION_IDLE_SECS;
use crate::db;

/// Build the auth sub-router: `/api/auth/*`.
///
/// Rate limiting: `/login` and `/callback` are limited to 10 req/min per IP
/// via `tower-governor`. Authenticated endpoints are not rate-limited here.
pub fn router() -> Router<AppState> {
    // Rate limit: 10 requests per 60 seconds per IP
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(60)
        .burst_size(10)
        .finish()
        .expect("failed to build governor config");
    let governor_layer = GovernorLayer::new(governor_conf).error_handler(|err| {
        tracing::error!("rate limiter: {err}");
        let (status, message) = match &err {
            GovernorError::TooManyRequests { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded")
            }
            GovernorError::UnableToExtractKey => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "unable to extract client IP",
            ),
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
    });

    // Rate-limited session lifecycle routes
    let oauth_routes = Router::new()
        .route("/login", get(login))
        .route("/callback", get(callback))
        .route("/logout", post(logout))
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
    is_robot: bool,
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

    // Dev mode: skip Google, redirect to callback with seed_admin email.
    if state.config.dev.enabled {
        let email = state.config.seed.seed_admin.as_ref().ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "dev mode requires seed-admin in config",
            )
        })?;
        let url = format!("/api/auth/callback?state={oauth_nonce}&dev_email={email}");
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

    // Read client_type before cycling the session
    let client_type: String = session
        .get("client_type")
        .await
        .map_err(|e| {
            tracing::error!("session get failed: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "session error")
        })?
        .unwrap_or_else(|| "web".to_string());

    // Resolve user info — dev mode or Google exchange.
    let user_info = if let Some(email) = params.dev_email.filter(|_| state.config.dev.enabled) {
        let name = email.split('@').next().unwrap_or(&email).to_string();
        oauth::GoogleUserInfo { email, name }
    } else {
        let code = params
            .code
            .ok_or_else(|| auth_error(StatusCode::BAD_REQUEST, "missing authorization code"))?;

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
            let domain = info.email.rsplit_once('@').map(|(_, d)| d).unwrap_or("");

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

    // Create or update user in DB. A display name starting with "robot:"
    // is rejected to prevent identity forgery via the OAuth display-name
    // field — only service accounts may hold that prefix, and only ever
    // via `POST /api/admin/robots`.
    let _user = db::users::create_or_update_user(&state.pool, &user_info.email, &user_info.name)
        .await
        .map_err(|e| match e {
            db::users::CreateOrUpdateUserError::RobotNamePrefix => {
                tracing::warn!(
                    email = %user_info.email,
                    "SSO forgery guard: rejecting sign-in with 'robot:'-prefixed display name",
                );
                auth_error(
                    StatusCode::FORBIDDEN,
                    "display name starting with 'robot:' is reserved for service accounts",
                )
            }
            db::users::CreateOrUpdateUserError::Db(err) => {
                tracing::error!("failed to create/update user: {err}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create user")
            }
        })?;

    // Token lifetime matches max-token-ttl-seconds (default: 6 months).
    let max_ttl = state.config.secrets.max_token_ttl_seconds;
    let expires_at = Some(chrono::Utc::now().timestamp() + max_ttl as i64);

    let (raw_token, token_hash) = paseto::token_create(
        &user_info.email,
        max_ttl,
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
        // CLI: redirect to UI with token in fragment for copy-paste.
        // The fragment is client-side only — never sent to the server.
        let redirect_url = format!("/?cli-token={token_b64}");
        Ok(Redirect::temporary(&redirect_url).into_response())
    } else {
        // Web: store token server-side in session (BFF pattern).
        // The browser gets a session cookie; the token never leaves
        // the server.
        session
            .insert("paseto_token", &raw_token)
            .await
            .map_err(|e| {
                tracing::error!("session insert failed: {e}");
                auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to store session")
            })?;
        session.set_expiry(Some(tower_sessions::Expiry::OnInactivity(
            time::Duration::seconds(WEB_SESSION_IDLE_SECS as i64),
        )));
        Ok(Redirect::temporary("/").into_response())
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
        is_robot: user.is_robot,
        roles,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/auth/token/revoke
// ---------------------------------------------------------------------------

/// Self-revoke: revokes the bearer token used in the current request.
/// Cookie-authenticated users should use POST /api/auth/logout instead.
async fn revoke_token(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Cookie-authenticated users don't have a bearer token to revoke.
    if headers.get("authorization").is_none() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "use POST /api/auth/logout for web sessions",
        ));
    }

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
async fn revoke_all_tokens(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<RevokeAllBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("permissions:manage") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: permissions:manage",
        ));
    }

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
// POST /api/auth/logout
// ---------------------------------------------------------------------------

/// Clear the web session and revoke the underlying PASETO token.
///
/// Does NOT use the `AuthUser` extractor — the session may contain an
/// expired or revoked token that fails validation. The user should
/// always be able to log out.
async fn logout(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Revoke the token if one is stored in the session.
    match session.get::<String>("paseto_token").await {
        Ok(Some(raw_token)) => {
            let hash = paseto::token_hash(&raw_token);
            if let Err(e) = db::tokens::revoke_token(&state.pool, &hash).await {
                tracing::warn!("logout: failed to revoke session token: {e}");
            }
        }
        Ok(None) => {} // no token stored — nothing to revoke
        Err(e) => tracing::warn!("logout: could not read session token: {e}"),
    }

    // Flush the session (deletes server-side data + clears the cookie).
    session.flush().await.map_err(|e| {
        tracing::error!("session flush failed: {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to clear session")
    })?;

    Ok(Json(serde_json::json!({"detail": "logged out"})))
}

// ---------------------------------------------------------------------------
// POST /api/auth/api-keys
// ---------------------------------------------------------------------------

async fn create_api_key_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("apikeys:create:own") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: apikeys:create:own",
        ));
    }

    // Reject reserved prefix — worker keys are managed via /api/admin/workers
    if body.name.starts_with("worker:") {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "key names starting with 'worker:' are reserved for worker registration",
        ));
    }

    let (key, prefix) = token_cache::create_api_key(&state.pool, &body.name, &user.email)
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
    if !user.has_cap("apikeys:create:own") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: apikeys:create:own",
        ));
    }

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
    if !user.has_cap("apikeys:create:own") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: apikeys:create:own",
        ));
    }

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
    let mut cache = state.token_cache.lock().await;
    cache.remove_by_prefix(&prefix);

    tracing::info!("revoked API key (prefix={}) for {}", prefix, user.email);
    Ok(Json(serde_json::json!({"detail": "API key revoked"})))
}
