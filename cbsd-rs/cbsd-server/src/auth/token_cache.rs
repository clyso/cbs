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

//! Unified LRU cache for API keys (`cbsk_`) and robot tokens (`cbrk_`).
//!
//! Both token types are keyed by SHA-256 of the raw bearer string for O(1)
//! repeated lookups. Cache miss triggers Argon2id verification against the
//! appropriate DB table.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock};

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use lru::LruCache;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::db;

/// Argon2id hash used as a timing sentinel on the no-candidate path of
/// [`verify_hashed_token`]. The plaintext is not secret — its only purpose
/// is to make [`argon2_verify`] do real CPU work on empty-result lookups,
/// so an attacker cannot distinguish "prefix exists in DB" from "prefix
/// absent" by timing the bearer path (prefix-enumeration defense).
///
/// Computed once on first access and reused thereafter. Must share the
/// same Argon2 parameters as real hashes (`Argon2::default()`) so the
/// verify cost matches.
static DUMMY_ARGON2_HASH: LazyLock<String> = LazyLock::new(|| {
    argon2_hash("cbsd-timing-parity-sentinel")
        .expect("argon2_hash of static sentinel string must succeed")
});

/// Discriminates between the two bearer-token types in the unified cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    ApiKey,
    RobotToken,
}

/// Cached token entry (stored after successful verification).
#[derive(Debug, Clone)]
pub struct CachedToken {
    /// Distinguishes API keys from robot tokens; used by token rotation (R2).
    #[allow(dead_code)]
    pub kind: TokenKind,
    pub token_id: i64,
    pub owner_email: String,
    pub prefix: String,
    pub expires_at: Option<i64>,
}

/// Unified LRU cache for API keys and robot tokens, keyed by SHA-256 of the
/// raw bearer string.
///
/// Reverse indexes by `owner_email` and `prefix` allow efficient invalidation
/// after revocation without scanning the whole cache.
pub struct TokenCache {
    by_sha256: LruCache<[u8; 32], CachedToken>,
    by_prefix: HashMap<String, HashSet<[u8; 32]>>,
    by_owner: HashMap<String, HashSet<[u8; 32]>>,
}

impl TokenCache {
    pub fn new(capacity: usize) -> Arc<Mutex<Self>> {
        let cap = NonZeroUsize::new(capacity).expect("cache capacity must be > 0");
        Arc::new(Mutex::new(Self {
            by_sha256: LruCache::new(cap),
            by_prefix: HashMap::new(),
            by_owner: HashMap::new(),
        }))
    }

    /// Insert a verified token into the cache. Handles LRU eviction cleanup.
    pub fn insert(&mut self, sha256: [u8; 32], entry: CachedToken) {
        // If LRU evicts an old entry, clean up its reverse indexes.
        if let Some((evicted_hash, evicted)) = self.by_sha256.push(sha256, entry.clone())
            && evicted_hash != sha256
        {
            Self::remove_from_reverse_maps(
                &mut self.by_prefix,
                &mut self.by_owner,
                &evicted.prefix,
                &evicted.owner_email,
                &evicted_hash,
            );
        }

        self.by_prefix
            .entry(entry.prefix.clone())
            .or_default()
            .insert(sha256);
        self.by_owner
            .entry(entry.owner_email.clone())
            .or_default()
            .insert(sha256);
    }

    /// Look up a cached token by its SHA-256 hash. Promotes in LRU on hit.
    pub fn get(&mut self, sha256: &[u8; 32]) -> Option<&CachedToken> {
        self.by_sha256.get(sha256)
    }

    /// Remove all cached entries matching a prefix (individual revocation).
    pub fn remove_by_prefix(&mut self, prefix: &str) {
        if let Some(hashes) = self.by_prefix.remove(prefix) {
            for h in &hashes {
                if let Some(entry) = self.by_sha256.pop(h)
                    && let Some(set) = self.by_owner.get_mut(&entry.owner_email)
                {
                    set.remove(h);
                    if set.is_empty() {
                        self.by_owner.remove(&entry.owner_email);
                    }
                }
            }
        }
    }

    /// Remove all cached entries for an owner (bulk deactivation / tombstone).
    pub fn remove_by_owner(&mut self, email: &str) {
        if let Some(hashes) = self.by_owner.remove(email) {
            for h in &hashes {
                if let Some(entry) = self.by_sha256.pop(h)
                    && let Some(set) = self.by_prefix.get_mut(&entry.prefix)
                {
                    set.remove(h);
                    if set.is_empty() {
                        self.by_prefix.remove(&entry.prefix);
                    }
                }
            }
        }
    }

    fn remove_from_reverse_maps(
        by_prefix: &mut HashMap<String, HashSet<[u8; 32]>>,
        by_owner: &mut HashMap<String, HashSet<[u8; 32]>>,
        prefix: &str,
        owner: &str,
        hash: &[u8; 32],
    ) {
        if let Some(set) = by_prefix.get_mut(prefix) {
            set.remove(hash);
            if set.is_empty() {
                by_prefix.remove(prefix);
            }
        }
        if let Some(set) = by_owner.get_mut(owner) {
            set.remove(hash);
            if set.is_empty() {
                by_owner.remove(owner);
            }
        }
    }
}

/// Error type for API key / robot token operations.
#[derive(Debug)]
pub enum TokenError {
    Db(sqlx::Error),
    InvalidFormat,
    NotFound,
    Expired,
    HashError(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::InvalidFormat => write!(f, "invalid token format"),
            Self::NotFound => write!(f, "token not found or revoked"),
            Self::Expired => write!(f, "token expired"),
            Self::HashError(e) => write!(f, "hashing error: {e}"),
        }
    }
}

impl std::error::Error for TokenError {}

impl From<sqlx::Error> for TokenError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

// ---------------------------------------------------------------------------
// Shared token plumbing for cbsk_ and cbrk_ bearer tokens
// ---------------------------------------------------------------------------

/// Candidate row returned by a prefix-indexed lookup, normalized so the
/// shared verify path doesn't care which table produced it. Both
/// `db::api_keys::ApiKeyRow` and `db::robots::TokenCandidate` map into
/// this shape via `From` impls.
#[derive(Debug, Clone)]
struct CandidateRow {
    id: i64,
    hash: String,
    prefix: String,
    owner_email: String,
    expires_at: Option<i64>,
}

impl From<db::api_keys::ApiKeyRow> for CandidateRow {
    fn from(r: db::api_keys::ApiKeyRow) -> Self {
        Self {
            id: r.id,
            hash: r.key_hash,
            prefix: r.key_prefix,
            owner_email: r.owner_email,
            expires_at: r.expires_at,
        }
    }
}

impl From<db::robots::TokenCandidate> for CandidateRow {
    fn from(r: db::robots::TokenCandidate) -> Self {
        Self {
            id: r.id,
            hash: r.token_hash,
            prefix: r.token_prefix,
            owner_email: r.robot_email,
            expires_at: r.expires_at,
        }
    }
}

/// Generate random bearer-token material with the given 5-char prefix.
/// Returns `(plaintext, lookup_prefix, argon2_hash)`. Plaintext format is
/// `<prefix><64 hex chars>`; lookup prefix is the first 12 hex chars after
/// the literal prefix.
async fn generate_token_material(
    prefix: &'static str,
) -> Result<(String, String, String), TokenError> {
    let random_bytes: [u8; 32] = rand::thread_rng().r#gen();
    let hex_part = hex_encode(&random_bytes);
    let raw = format!("{prefix}{hex_part}");
    let lookup_prefix = raw[prefix.len()..prefix.len() + 12].to_string();

    let raw_clone = raw.clone();
    let hash = tokio::task::spawn_blocking(move || argon2_hash(&raw_clone))
        .await
        .map_err(|e| TokenError::HashError(e.to_string()))??;

    Ok((raw, lookup_prefix, hash))
}

/// Shared verify plumbing for both `cbsk_` and `cbrk_` paths.
///
/// Flow: length + prefix sanity, SHA-256 cache lookup, prefix-indexed DB
/// fetch via the supplied closure, Argon2id verify under spawn_blocking,
/// expiry check, cache insert.
async fn verify_hashed_token<Fetch, Fut>(
    cache: &Arc<Mutex<TokenCache>>,
    raw: &str,
    expected_prefix: &'static str,
    kind: TokenKind,
    fetch_candidates: Fetch,
) -> Result<CachedToken, TokenError>
where
    Fetch: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<CandidateRow>, sqlx::Error>>,
{
    if raw.len() != 69 || !raw.starts_with(expected_prefix) {
        return Err(TokenError::InvalidFormat);
    }

    let sha256: [u8; 32] = Sha256::digest(raw.as_bytes()).into();

    // Cache hit: verify expiry and return.
    {
        let mut guard = cache.lock().await;
        if let Some(cached) = guard.get(&sha256) {
            let cached = cached.clone();
            if let Some(exp) = cached.expires_at
                && chrono::Utc::now().timestamp() > exp
            {
                return Err(TokenError::Expired);
            }
            return Ok(cached);
        }
    }

    let lookup_prefix = raw[expected_prefix.len()..expected_prefix.len() + 12].to_string();
    let candidates = fetch_candidates(lookup_prefix).await?;
    if candidates.is_empty() {
        // Timing parity: run a dummy Argon2 verify so the no-match path
        // takes the same wall-clock time as the match path. Without this,
        // an attacker can distinguish "prefix exists (~250ms)" from
        // "prefix absent (~1ms)" and enumerate active token prefixes.
        let raw_owned = raw.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            argon2_verify(&raw_owned, &DUMMY_ARGON2_HASH);
        })
        .await;
        return Err(TokenError::NotFound);
    }

    // Argon2id is CPU-bound: verify off the async executor.
    let raw_owned = raw.to_string();
    let verified = tokio::task::spawn_blocking(move || {
        for c in &candidates {
            if argon2_verify(&raw_owned, &c.hash) {
                return Some(c.clone());
            }
        }
        None
    })
    .await
    .map_err(|e| TokenError::HashError(e.to_string()))?;

    let c = verified.ok_or(TokenError::NotFound)?;
    let entry = CachedToken {
        kind,
        token_id: c.id,
        owner_email: c.owner_email,
        prefix: c.prefix,
        expires_at: c.expires_at,
    };

    if let Some(exp) = entry.expires_at
        && chrono::Utc::now().timestamp() > exp
    {
        return Err(TokenError::Expired);
    }

    {
        let mut guard = cache.lock().await;
        guard.insert(sha256, entry.clone());
    }

    Ok(entry)
}

// ---------------------------------------------------------------------------
// API key (cbsk_) operations
// ---------------------------------------------------------------------------

/// Generate a new API key, hash it, and store it in the database.
///
/// Returns `(plaintext_key, prefix)`. Plaintext is shown once, never stored.
pub async fn create_api_key(
    pool: &SqlitePool,
    name: &str,
    owner_email: &str,
) -> Result<(String, String), TokenError> {
    let (raw_key, prefix, hash) = generate_api_key_material().await?;
    db::api_keys::insert_api_key(pool, name, owner_email, &hash, &prefix).await?;
    Ok((raw_key, prefix))
}

/// Verify a raw API key. Checks the LRU cache first; falls back to DB lookup
/// + Argon2id verification on cache miss.
pub async fn verify_api_key(
    pool: &SqlitePool,
    cache: &Arc<Mutex<TokenCache>>,
    raw_key: &str,
) -> Result<CachedToken, TokenError> {
    verify_hashed_token(
        cache,
        raw_key,
        "cbsk_",
        TokenKind::ApiKey,
        |prefix| async move {
            let rows = db::api_keys::find_api_keys_by_prefix(pool, &prefix).await?;
            Ok(rows.into_iter().map(CandidateRow::from).collect())
        },
    )
    .await
}

/// Generate random API key material: `(plaintext, prefix, argon2_hash)`.
///
/// Performs Argon2id hashing in a blocking thread. The caller inserts the row
/// into the DB (via pool or transaction). This keeps Argon2 off the async
/// executor and out of any open transaction.
pub async fn generate_api_key_material() -> Result<(String, String, String), TokenError> {
    generate_token_material("cbsk_").await
}

// ---------------------------------------------------------------------------
// Robot token (cbrk_) operations
// ---------------------------------------------------------------------------

/// Generate random robot token material: `(plaintext, prefix, argon2_hash)`.
///
/// Format: `cbrk_<64 hex chars>`. Prefix = first 12 hex chars after `cbrk_`.
/// Caller is responsible for DB insertion (pool or transaction).
pub async fn generate_robot_token_material() -> Result<(String, String, String), TokenError> {
    generate_token_material("cbrk_").await
}

/// Verify a raw robot token against the `robot_tokens` table.
/// Checks the LRU cache first; falls back to DB lookup + Argon2id on miss.
pub async fn verify_robot_token(
    pool: &SqlitePool,
    cache: &Arc<Mutex<TokenCache>>,
    raw_token: &str,
) -> Result<CachedToken, TokenError> {
    verify_hashed_token(
        cache,
        raw_token,
        "cbrk_",
        TokenKind::RobotToken,
        |prefix| async move {
            let rows = db::robots::find_active_token_by_prefix(pool, &prefix).await?;
            Ok(rows.into_iter().map(CandidateRow::from).collect())
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Shared hashing helpers
// ---------------------------------------------------------------------------

/// Hash a raw token with Argon2id (default params).
fn argon2_hash(raw: &str) -> Result<String, TokenError> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(raw.as_bytes(), &salt)
        .map_err(|e| TokenError::HashError(e.to_string()))?;
    Ok(hash.to_string())
}

/// Verify a raw token against an Argon2 hash string.
fn argon2_verify(raw: &str, hash_str: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash_str) else {
        return false;
    };
    Argon2::default()
        .verify_password(raw.as_bytes(), &parsed)
        .is_ok()
}

/// Lowercase hex encode. Exposed for use by `db::seed`.
#[allow(dead_code)]
pub(crate) fn hex_encode_bytes(bytes: &[u8]) -> String {
    hex_encode(bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dummy_argon2_hash_initialises_and_rejects_non_sentinel_inputs() {
        let h = &*DUMMY_ARGON2_HASH;
        assert!(
            PasswordHash::new(h).is_ok(),
            "sentinel must parse as a valid Argon2 PHC string"
        );
        assert!(
            !argon2_verify("not-the-sentinel", h),
            "sentinel must reject any non-sentinel plaintext so the \
             dummy verify on the no-candidate timing-parity path is \
             indistinguishable from a failed real verify"
        );
    }
}
