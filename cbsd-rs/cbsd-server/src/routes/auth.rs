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

//! Auth route handlers: OAuth login and callback.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_sessions::Session;

use crate::app::AppState;
use crate::auth::extractors::{auth_error, ErrorDetail};
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
            .finish()
            .expect("failed to build governor config"),
    );
    let governor_layer = GovernorLayer {
        config: governor_conf,
    };

    // Rate-limited OAuth routes
    Router::new()
        .route("/login", get(login))
        .route("/callback", get(callback))
        .layer(governor_layer)
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
    code: String,
    state: String,
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
    session.insert("oauth_state", &oauth_nonce).await.map_err(|e| {
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
        return Err(auth_error(StatusCode::BAD_REQUEST, "missing OAuth state in session"));
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

    // Exchange authorization code for user info
    let user_info = oauth::exchange_code_for_userinfo(&state.oauth, &params.code)
        .await
        .map_err(|e| {
            tracing::error!("OAuth code exchange failed: {e}");
            auth_error(StatusCode::BAD_GATEWAY, "failed to authenticate with Google")
        })?;

    // Check domain restriction
    if !state.config.oauth.allow_any_google_account {
        let domain = user_info
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
