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
/// `max_ttl_secs` controls both the application-level expiry in the
/// payload and the PASETO-level `exp` claim. The PASETO `exp` acts
/// as a hard ceiling enforced by the `pasetors` library on decrypt.
///
/// Returns `(raw_token_string, sha256_hex_hash)`.
pub fn token_create(
    email: &str,
    max_ttl_secs: u64,
    secret_key_hex: &str,
) -> Result<(String, String), TokenError> {
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + max_ttl_secs as i64;

    let payload = CbsdTokenPayloadV1 {
        expires: Some(expires_at),
        user: email.to_string(),
    };

    // Canonical JSON: keys alphabetically ordered (struct field order matches),
    // no whitespace. serde_json serializes struct fields in declaration order.
    let payload_json = serde_json::to_string(&payload).map_err(|_| TokenError::Serialization)?;

    let key_bytes = hex::decode(secret_key_hex).map_err(|_| TokenError::InvalidKey)?;
    let sym_key =
        SymmetricKey::<V4>::from(key_bytes.as_slice()).map_err(|_| TokenError::InvalidKey)?;

    // Set the PASETO-level expiry to match the application TTL.
    let ttl_duration = std::time::Duration::from_secs(max_ttl_secs);
    let mut claims =
        Claims::new_expires_in(&ttl_duration).map_err(|_| TokenError::Creation)?;
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
        let (token, hash) = token_create("alice@clyso.com", 3600, TEST_KEY).unwrap();
        assert!(token.starts_with("v4.local."));
        assert_eq!(hash.len(), 64); // SHA-256 hex

        let payload = token_decode(&token, TEST_KEY).unwrap();
        assert_eq!(payload.user, "alice@clyso.com");
        assert!(payload.expires.is_some());
    }

    #[test]
    fn create_with_ttl() {
        let now = chrono::Utc::now().timestamp();
        let (token, _) = token_create("bob@clyso.com", 7200, TEST_KEY).unwrap();
        let payload = token_decode(&token, TEST_KEY).unwrap();
        let exp = payload.expires.unwrap();
        assert!(exp >= now + 7199);
        assert!(exp <= now + 7201);
    }

    #[test]
    fn wrong_key_rejected() {
        let (token, _) = token_create("alice@clyso.com", 3600, TEST_KEY).unwrap();
        let other_key = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert!(matches!(
            token_decode(&token, other_key),
            Err(TokenError::InvalidToken)
        ));
    }

    #[test]
    fn token_hash_deterministic() {
        let (token, hash1) = token_create("alice@clyso.com", 3600, TEST_KEY).unwrap();
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
        assert_eq!(json, r#"{"expires":1710412200,"user":"alice@clyso.com"}"#);
    }
}
