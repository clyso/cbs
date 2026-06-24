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

impl OAuthState {
    /// Create a dummy OAuthState for dev mode. The values are never
    /// used — dev mode bypasses Google entirely.
    pub fn dummy() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: String::new(),
        }
    }
}

/// User information returned by Google's userinfo endpoint.
///
/// `email_verified` is canonical per OpenID Connect; Google's v2
/// `/oauth2/v2/userinfo` endpoint returns `verified_email` — accepted
/// here as a serde alias for the legacy field name. Missing field
/// defaults to `false` so an unverified or malformed response is
/// rejected by [`validate_user_info`].
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleUserInfo {
    pub email: String,
    pub name: String,
    #[serde(default, alias = "verified_email")]
    pub email_verified: bool,
}

/// Reasons why a `GoogleUserInfo` may be rejected during the OAuth
/// callback. Surfaced to the user as a single generic error to avoid
/// leaking which check failed (audit-rem D2 pitfall).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRejection {
    /// Google reported the email is not verified (or the field was
    /// absent in the userinfo response).
    EmailNotVerified,
    /// The email's domain is not in `allowed_domains` and
    /// `allow_any_google_account` is false.
    DomainNotAllowed,
}

impl std::fmt::Display for AuthRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmailNotVerified => write!(f, "email not verified"),
            Self::DomainNotAllowed => write!(f, "email domain not allowed"),
        }
    }
}

/// Whether an email's domain is permitted by the OAuth allow-list. Shared by
/// the login path ([`validate_user_info`]) and user provisioning (design 020),
/// so a provisioned address is held to the same domain policy as a login.
/// `allow_any_google_account` short-circuits to `true`. The email is assumed
/// already normalized (lowercase); the allow-list is normalized at config load.
pub fn is_email_domain_allowed(
    email: &str,
    allowed_domains: &[String],
    allow_any_google_account: bool,
) -> bool {
    if allow_any_google_account {
        return true;
    }
    let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
    allowed_domains.iter().any(|d| d == domain)
}

/// Validate the userinfo against the server's OAuth config. Returns
/// `Err(AuthRejection::EmailNotVerified)` first so an attacker cannot
/// probe `allowed_domains` with unverified accounts (audit-rem D2).
pub fn validate_user_info(
    info: &GoogleUserInfo,
    allowed_domains: &[String],
    allow_any_google_account: bool,
) -> Result<(), AuthRejection> {
    if !info.email_verified {
        return Err(AuthRejection::EmailNotVerified);
    }
    if !is_email_domain_allowed(&info.email, allowed_domains, allow_any_google_account) {
        return Err(AuthRejection::DomainNotAllowed);
    }
    Ok(())
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
    Config(String),
    /// HTTP request to Google failed.
    Request(String),
    /// Google returned an error response.
    Google(String),
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "OAuth config error: {e}"),
            Self::Request(e) => write!(f, "OAuth request error: {e}"),
            Self::Google(e) => write!(f, "Google OAuth error: {e}"),
        }
    }
}

impl std::error::Error for OAuthError {}

/// Load Google OAuth configuration from a secrets JSON file.
pub fn load_oauth_config(path: &Path) -> Result<OAuthState, OAuthError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| OAuthError::Config(format!("failed to read {}: {e}", path.display())))?;

    let secrets: GoogleSecretsFile = serde_json::from_str(&contents)
        .map_err(|e| OAuthError::Config(format!("failed to parse secrets JSON: {e}")))?;

    let redirect_uri = secrets
        .web
        .redirect_uris
        .into_iter()
        .next()
        .ok_or_else(|| OAuthError::Config("no redirect_uris in secrets file".to_string()))?;

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
        .map_err(|e| OAuthError::Request(e.to_string()))?;

    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(OAuthError::Google(format!("token exchange failed: {body}")));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }

    let token: TokenResponse = token_resp
        .json()
        .await
        .map_err(|e| OAuthError::Google(format!("failed to parse token response: {e}")))?;

    // Step 2: Fetch user info
    let userinfo_resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(&token.access_token)
        .send()
        .await
        .map_err(|e| OAuthError::Request(e.to_string()))?;

    if !userinfo_resp.status().is_success() {
        let body = userinfo_resp.text().await.unwrap_or_default();
        return Err(OAuthError::Google(format!(
            "userinfo request failed: {body}"
        )));
    }

    let user_info: GoogleUserInfo = userinfo_resp
        .json()
        .await
        .map_err(|e| OAuthError::Google(format!("failed to parse userinfo: {e}")))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(email: &str, verified: bool) -> GoogleUserInfo {
        GoogleUserInfo {
            email: email.to_string(),
            name: "Test User".to_string(),
            email_verified: verified,
        }
    }

    #[test]
    fn deserializes_email_verified_canonical() {
        let json = r#"{"email": "u@example.com", "name": "U", "email_verified": true}"#;
        let v: GoogleUserInfo = serde_json::from_str(json).expect("parse");
        assert!(v.email_verified);
    }

    #[test]
    fn deserializes_verified_email_legacy_alias() {
        // Google's v2 /oauth2/v2/userinfo endpoint returns this name.
        let json = r#"{"email": "u@example.com", "name": "U", "verified_email": true}"#;
        let v: GoogleUserInfo = serde_json::from_str(json).expect("parse");
        assert!(v.email_verified);
    }

    #[test]
    fn missing_field_defaults_to_unverified() {
        let json = r#"{"email": "u@example.com", "name": "U"}"#;
        let v: GoogleUserInfo = serde_json::from_str(json).expect("parse");
        assert!(!v.email_verified);
    }

    #[test]
    fn validate_rejects_unverified_email_before_domain_check() {
        // Attacker's email_verified=false; domain happens to be in the
        // allow-list. The verification check must fire first so the
        // domain allow-list is not probed.
        let v = info("evil@allowed.example.com", false);
        let result = validate_user_info(&v, &["allowed.example.com".to_string()], false);
        assert_eq!(result, Err(AuthRejection::EmailNotVerified));
    }

    #[test]
    fn validate_accepts_verified_and_allowed() {
        let v = info("ok@allowed.example.com", true);
        let result = validate_user_info(&v, &["allowed.example.com".to_string()], false);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn validate_rejects_verified_but_disallowed_domain() {
        let v = info("ok@other.example.com", true);
        let result = validate_user_info(&v, &["allowed.example.com".to_string()], false);
        assert_eq!(result, Err(AuthRejection::DomainNotAllowed));
    }

    #[test]
    fn validate_accepts_when_allow_any_and_verified() {
        let v = info("anyone@anywhere.com", true);
        let result = validate_user_info(&v, &[], true);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn validate_rejects_unverified_even_when_allow_any() {
        let v = info("anyone@anywhere.com", false);
        let result = validate_user_info(&v, &[], true);
        assert_eq!(result, Err(AuthRejection::EmailNotVerified));
    }

    #[test]
    fn domain_allowed_matches_list_and_short_circuits_on_allow_any() {
        // Exercised directly by user provisioning, which has no
        // email_verified gate (design 020).
        let allowed = ["allowed.example.com".to_string()];
        assert!(is_email_domain_allowed(
            "ok@allowed.example.com",
            &allowed,
            false
        ));
        assert!(!is_email_domain_allowed(
            "ok@other.example.com",
            &allowed,
            false
        ));
        // No '@' yields an empty domain that matches nothing.
        assert!(!is_email_domain_allowed("malformed", &allowed, false));
        // allow_any short-circuits regardless of the allow-list.
        assert!(is_email_domain_allowed("anyone@anywhere.com", &[], true));
    }
}
