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

//! Google OAuth2 protocol helpers.
//!
//! Handles reading the Google client secrets JSON, building authorization URLs,
//! and exchanging authorization codes for user info.

use std::path::Path;

use serde::Deserialize;

/// Google OAuth client configuration loaded from the secrets JSON file.
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

/// User information returned by Google's userinfo endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleUserInfo {
    pub email: String,
    pub name: String,
}

/// Layout of the Google OAuth secrets JSON file.
#[derive(Deserialize)]
struct GoogleSecretsFile {
    web: GoogleWebSecrets,
}

#[derive(Deserialize)]
struct GoogleWebSecrets {
    client_id: String,
    client_secret: String,
    redirect_uris: Vec<String>,
}

/// Error type for OAuth operations.
#[derive(Debug)]
pub enum OAuthError {
    /// Failed to read or parse the secrets file.
    ConfigError(String),
    /// HTTP request to Google failed.
    RequestError(String),
    /// Google returned an error response.
    GoogleError(String),
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigError(e) => write!(f, "OAuth config error: {e}"),
            Self::RequestError(e) => write!(f, "OAuth request error: {e}"),
            Self::GoogleError(e) => write!(f, "Google OAuth error: {e}"),
        }
    }
}

impl std::error::Error for OAuthError {}

/// Load Google OAuth configuration from a secrets JSON file.
pub fn load_oauth_config(path: &Path) -> Result<OAuthState, OAuthError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| OAuthError::ConfigError(format!("failed to read {}: {e}", path.display())))?;

    let secrets: GoogleSecretsFile = serde_json::from_str(&contents)
        .map_err(|e| OAuthError::ConfigError(format!("failed to parse secrets JSON: {e}")))?;

    let redirect_uri = secrets
        .web
        .redirect_uris
        .into_iter()
        .next()
        .ok_or_else(|| OAuthError::ConfigError("no redirect_uris in secrets file".to_string()))?;

    Ok(OAuthState {
        client_id: secrets.web.client_id,
        client_secret: secrets.web.client_secret,
        redirect_uri,
    })
}

/// Build the Google OAuth2 authorization URL.
///
/// - `oauth_state`: random nonce for CSRF protection (stored in session).
/// - `hd`: optional hosted domain hint (restricts account picker).
pub fn build_google_auth_url(state: &OAuthState, oauth_state: &str, hd: Option<&str>) -> String {
    let mut url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id={client_id}\
         &redirect_uri={redirect_uri}\
         &response_type=code\
         &scope=openid%20email%20profile\
         &access_type=offline\
         &state={state_param}",
        client_id = urlencoding(state.client_id.as_str()),
        redirect_uri = urlencoding(state.redirect_uri.as_str()),
        state_param = urlencoding(oauth_state),
    );

    if let Some(domain) = hd {
        url.push_str(&format!("&hd={}", urlencoding(domain)));
    }

    url
}

/// Exchange an authorization code for Google user info.
///
/// 1. POST to Google's token endpoint to get an access token.
/// 2. GET Google's userinfo endpoint with the access token.
pub async fn exchange_code_for_userinfo(
    state: &OAuthState,
    code: &str,
) -> Result<GoogleUserInfo, OAuthError> {
    let client = reqwest::Client::new();

    // Step 1: Exchange code for access token
    let token_resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code),
            ("client_id", &state.client_id),
            ("client_secret", &state.client_secret),
            ("redirect_uri", &state.redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| OAuthError::RequestError(e.to_string()))?;

    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(OAuthError::GoogleError(format!(
            "token exchange failed: {body}"
        )));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }

    let token: TokenResponse = token_resp
        .json()
        .await
        .map_err(|e| OAuthError::GoogleError(format!("failed to parse token response: {e}")))?;

    // Step 2: Fetch user info
    let userinfo_resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(&token.access_token)
        .send()
        .await
        .map_err(|e| OAuthError::RequestError(e.to_string()))?;

    if !userinfo_resp.status().is_success() {
        let body = userinfo_resp.text().await.unwrap_or_default();
        return Err(OAuthError::GoogleError(format!(
            "userinfo request failed: {body}"
        )));
    }

    let user_info: GoogleUserInfo = userinfo_resp
        .json()
        .await
        .map_err(|e| OAuthError::GoogleError(format!("failed to parse userinfo: {e}")))?;

    Ok(user_info)
}

/// Minimal percent-encoding for URL query parameter values.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(char::from(HEX_CHARS[(b >> 4) as usize]));
                out.push(char::from(HEX_CHARS[(b & 0x0f) as usize]));
            }
        }
    }
    out
}

const HEX_CHARS: [u8; 16] = *b"0123456789ABCDEF";
