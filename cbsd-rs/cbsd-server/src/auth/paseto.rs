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

//! PASETO v4.local token creation and decoding.
//!
//! Frozen payload schema `CBSD_TOKEN_PAYLOAD_V1`:
//! ```json
//! {"expires":1710412200,"user":"alice@clyso.com"}
//! ```
//! - Keys alphabetically ordered, no whitespace.
//! - `expires`: Unix epoch seconds (i64) or null (infinite).
//! - `user`: email address.

use pasetors::claims::{Claims, ClaimsValidationRules};
use pasetors::keys::SymmetricKey;
use pasetors::token::UntrustedToken;
use pasetors::version4::V4;
use pasetors::{Local, local};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Frozen payload schema. Both Python and Rust must produce identical
/// canonical JSON for the same logical payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CbsdTokenPayloadV1 {
    /// Expiration as Unix epoch seconds. `None` = infinite TTL.
    pub expires: Option<i64>,
    /// User email address.
    pub user: String,
}

/// Create a PASETO v4.local token.
///
/// Returns `(raw_token_string, sha256_hex_hash)`.
pub fn token_create(
    email: &str,
    expires_at: Option<i64>,
    max_ttl: Option<u64>,
    secret_key_hex: &str,
) -> Result<(String, String), TokenError> {
    // Clamp TTL if max_token_ttl_seconds is set
    let expires_at = match (expires_at, max_ttl) {
        (Some(exp), Some(max)) => {
            let now = chrono::Utc::now().timestamp();
            let ttl = exp - now;
            if ttl > max as i64 {
                Some(now + max as i64)
            } else {
                Some(exp)
            }
        }
        (exp, _) => exp,
    };

    let payload = CbsdTokenPayloadV1 {
        expires: expires_at,
        user: email.to_string(),
    };

    // Canonical JSON: keys alphabetically ordered (struct field order matches),
    // no whitespace. serde_json serializes struct fields in declaration order.
    let payload_json = serde_json::to_string(&payload).map_err(|_| TokenError::Serialization)?;

    let key_bytes = hex::decode(secret_key_hex).map_err(|_| TokenError::InvalidKey)?;
    let sym_key =
        SymmetricKey::<V4>::from(key_bytes.as_slice()).map_err(|_| TokenError::InvalidKey)?;

    let mut claims = Claims::new().map_err(|_| TokenError::Creation)?;
    claims
        .add_additional("payload", payload_json.clone())
        .map_err(|_| TokenError::Creation)?;

    let raw_token = local::encrypt(&sym_key, &claims, None, None).map_err(|e| {
        tracing::error!("PASETO encrypt failed: {e}");
        TokenError::Creation
    })?;

    let hash = hex::encode(Sha256::digest(raw_token.as_bytes()));

    Ok((raw_token, hash))
}

/// Decode and validate a PASETO v4.local token.
pub fn token_decode(
    raw_token: &str,
    secret_key_hex: &str,
) -> Result<CbsdTokenPayloadV1, TokenError> {
    let key_bytes = hex::decode(secret_key_hex).map_err(|_| TokenError::InvalidKey)?;
    let sym_key =
        SymmetricKey::<V4>::from(key_bytes.as_slice()).map_err(|_| TokenError::InvalidKey)?;

    let untrusted =
        UntrustedToken::<Local, V4>::try_from(raw_token).map_err(|_| TokenError::InvalidToken)?;

    let validation_rules = ClaimsValidationRules::new();
    let trusted = local::decrypt(&sym_key, &untrusted, &validation_rules, None, None)
        .map_err(|_| TokenError::InvalidToken)?;

    let claims_json = trusted.payload_claims().ok_or(TokenError::InvalidToken)?;

    let payload_str = claims_json
        .get_claim("payload")
        .ok_or(TokenError::InvalidToken)?;

    // The payload claim is a JSON string containing our canonical JSON
    let payload_str = payload_str.as_str().ok_or(TokenError::InvalidToken)?;

    let payload: CbsdTokenPayloadV1 =
        serde_json::from_str(payload_str).map_err(|_| TokenError::InvalidToken)?;

    // Check expiry
    if let Some(exp) = payload.expires {
        let now = chrono::Utc::now().timestamp();
        if now > exp {
            return Err(TokenError::Expired);
        }
    }

    Ok(payload)
}

/// Compute SHA-256 hash of a raw PASETO token string.
pub fn token_hash(raw_token: &str) -> String {
    hex::encode(Sha256::digest(raw_token.as_bytes()))
}

#[derive(Debug, Clone)]
pub enum TokenError {
    Serialization,
    InvalidKey,
    Creation,
    InvalidToken,
    Expired,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization => write!(f, "payload serialization failed"),
            Self::InvalidKey => write!(f, "invalid secret key"),
            Self::Creation => write!(f, "token creation failed"),
            Self::InvalidToken => write!(f, "invalid or corrupted token"),
            Self::Expired => write!(f, "token expired"),
        }
    }
}

impl std::error::Error for TokenError {}

/// Hex encode/decode utility (avoids external `hex` crate dependency by
/// using a minimal implementation).
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if s.len() % 2 != 0 {
            return Err(());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 32-byte hex key for testing (64 hex chars = 32 bytes, PASETO v4 requirement)
    const TEST_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn create_and_decode_round_trip() {
        let (token, hash) = token_create("alice@clyso.com", None, None, TEST_KEY).unwrap();
        assert!(token.starts_with("v4.local."));
        assert_eq!(hash.len(), 64); // SHA-256 hex

        let payload = token_decode(&token, TEST_KEY).unwrap();
        assert_eq!(payload.user, "alice@clyso.com");
        assert_eq!(payload.expires, None);
    }

    #[test]
    fn create_with_expiry() {
        let future = chrono::Utc::now().timestamp() + 3600;
        let (token, _) = token_create("bob@clyso.com", Some(future), None, TEST_KEY).unwrap();
        let payload = token_decode(&token, TEST_KEY).unwrap();
        assert_eq!(payload.expires, Some(future));
    }

    #[test]
    fn expired_token_rejected() {
        let past = chrono::Utc::now().timestamp() - 3600;
        let (token, _) = token_create("bob@clyso.com", Some(past), None, TEST_KEY).unwrap();
        assert!(matches!(
            token_decode(&token, TEST_KEY),
            Err(TokenError::Expired)
        ));
    }

    #[test]
    fn wrong_key_rejected() {
        let (token, _) = token_create("alice@clyso.com", None, None, TEST_KEY).unwrap();
        let other_key = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert!(matches!(
            token_decode(&token, other_key),
            Err(TokenError::InvalidToken)
        ));
    }

    #[test]
    fn token_hash_deterministic() {
        let (token, hash1) = token_create("alice@clyso.com", None, None, TEST_KEY).unwrap();
        let hash2 = token_hash(&token);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn canonical_json_field_order() {
        let payload = CbsdTokenPayloadV1 {
            expires: Some(1710412200),
            user: "alice@clyso.com".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        // Keys must be alphabetical: expires before user
        assert_eq!(json, r#"{"expires":1710412200,"user":"alice@clyso.com"}"#);
    }

    #[test]
    fn canonical_json_null_expires() {
        let payload = CbsdTokenPayloadV1 {
            expires: None,
            user: "alice@clyso.com".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(json, r#"{"expires":null,"user":"alice@clyso.com"}"#);
    }

    #[test]
    fn max_ttl_clamping() {
        let far_future = chrono::Utc::now().timestamp() + 86400 * 365; // 1 year
        let (token, _) =
            token_create("alice@clyso.com", Some(far_future), Some(3600), TEST_KEY).unwrap();
        let payload = token_decode(&token, TEST_KEY).unwrap();
        let now = chrono::Utc::now().timestamp();
        // Clamped to ~3600s from now
        assert!(payload.expires.unwrap() <= now + 3601);
        assert!(payload.expires.unwrap() >= now + 3599);
    }
}
