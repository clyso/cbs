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

//! API key generation, hashing, and LRU-cache verification.
//!
//! Key format: `cbsk_<64 hex chars>` (32 random bytes).
//! Prefix: first 12 hex chars after `cbsk_` (chars 5..17).
//!
//! Cache keyed by SHA-256 of the raw key string for O(1) repeated lookups.
//! Cache miss triggers argon2 verification against DB rows (expensive).

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use lru::LruCache;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::db;

/// Cached API key entry (stored after successful verification).
#[derive(Debug, Clone)]
pub struct CachedApiKey {
    pub owner_email: String,
    pub key_prefix: String,
    pub expires_at: Option<i64>,
}

/// LRU cache for verified API keys, keyed by SHA-256 of the raw key string.
///
/// Maintains reverse indexes for efficient invalidation by prefix or owner.
pub struct ApiKeyCache {
    by_sha256: LruCache<[u8; 32], CachedApiKey>,
    by_prefix: HashMap<String, HashSet<[u8; 32]>>,
    by_owner: HashMap<String, HashSet<[u8; 32]>>,
}

impl ApiKeyCache {
    pub fn new(capacity: usize) -> Arc<Mutex<Self>> {
        let cap = NonZeroUsize::new(capacity).expect("cache capacity must be > 0");
        Arc::new(Mutex::new(Self {
            by_sha256: LruCache::new(cap),
            by_prefix: HashMap::new(),
            by_owner: HashMap::new(),
        }))
    }

    /// Insert a verified key into the cache. Handles LRU eviction cleanup.
    pub fn insert(&mut self, sha256: [u8; 32], entry: CachedApiKey) {
        // If LRU evicts an old entry, clean up its reverse indexes.
        if let Some((evicted_hash, evicted)) = self.by_sha256.push(sha256, entry.clone()) {
            if evicted_hash != sha256 {
                Self::remove_from_reverse_maps(
                    &mut self.by_prefix,
                    &mut self.by_owner,
                    &evicted.key_prefix,
                    &evicted.owner_email,
                    &evicted_hash,
                );
            }
        }

        self.by_prefix
            .entry(entry.key_prefix.clone())
            .or_default()
            .insert(sha256);
        self.by_owner
            .entry(entry.owner_email.clone())
            .or_default()
            .insert(sha256);
    }

    /// Look up a cached key by its SHA-256 hash. Promotes in LRU on hit.
    pub fn get(&mut self, sha256: &[u8; 32]) -> Option<&CachedApiKey> {
        self.by_sha256.get(sha256)
    }

    /// Remove all cached entries matching a key prefix (individual revocation).
    pub fn remove_by_prefix(&mut self, prefix: &str) {
        if let Some(hashes) = self.by_prefix.remove(prefix) {
            for h in &hashes {
                if let Some(entry) = self.by_sha256.pop(h) {
                    if let Some(set) = self.by_owner.get_mut(&entry.owner_email) {
                        set.remove(h);
                        if set.is_empty() {
                            self.by_owner.remove(&entry.owner_email);
                        }
                    }
                }
            }
        }
    }

    /// Remove all cached entries for an owner (bulk deactivation).
    pub fn remove_by_owner(&mut self, email: &str) {
        if let Some(hashes) = self.by_owner.remove(email) {
            for h in &hashes {
                if let Some(entry) = self.by_sha256.pop(h) {
                    if let Some(set) = self.by_prefix.get_mut(&entry.key_prefix) {
                        set.remove(h);
                        if set.is_empty() {
                            self.by_prefix.remove(&entry.key_prefix);
                        }
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

/// Error type for API key operations.
#[derive(Debug)]
pub enum ApiKeyError {
    /// Database error.
    Db(sqlx::Error),
    /// Invalid key format.
    InvalidFormat,
    /// Key not found or verification failed.
    NotFound,
    /// Key has expired.
    Expired,
    /// Argon2 hashing failed.
    HashError(String),
}

impl std::fmt::Display for ApiKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::InvalidFormat => write!(f, "invalid API key format"),
            Self::NotFound => write!(f, "API key not found or revoked"),
            Self::Expired => write!(f, "API key expired"),
            Self::HashError(e) => write!(f, "hashing error: {e}"),
        }
    }
}

impl std::error::Error for ApiKeyError {}

impl From<sqlx::Error> for ApiKeyError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// Generate a new API key, hash it with argon2, and store in the database.
///
/// Returns `(plaintext_key, prefix)`. The plaintext key is shown once to the
/// user and never stored.
pub async fn create_api_key(
    pool: &SqlitePool,
    name: &str,
    owner_email: &str,
) -> Result<(String, String), ApiKeyError> {
    // Generate 32 random bytes -> 64 hex chars
    let random_bytes: [u8; 32] = rand::thread_rng().r#gen();
    let hex_part = hex_encode(&random_bytes);
    let raw_key = format!("cbsk_{hex_part}");

    // Prefix = first 12 hex chars (chars 5..17 of the raw key)
    let prefix = raw_key[5..17].to_string();

    // Argon2 hash (expensive — run in blocking thread)
    let key_clone = raw_key.clone();
    let hash = tokio::task::spawn_blocking(move || argon2_hash(&key_clone))
        .await
        .map_err(|e| ApiKeyError::HashError(e.to_string()))??;

    db::api_keys::insert_api_key(pool, name, owner_email, &hash, &prefix).await?;

    Ok((raw_key, prefix))
}

/// Verify a raw API key. Checks the LRU cache first (by SHA-256), falls back
/// to DB lookup + argon2 verification on cache miss.
pub async fn verify_api_key(
    pool: &SqlitePool,
    cache: &Arc<Mutex<ApiKeyCache>>,
    raw_key: &str,
) -> Result<CachedApiKey, ApiKeyError> {
    // Validate format: cbsk_ + 64 hex chars = 69 chars
    if raw_key.len() != 69 || !raw_key.starts_with("cbsk_") {
        return Err(ApiKeyError::InvalidFormat);
    }

    // SHA-256 of the raw key for cache lookup
    let sha256: [u8; 32] = Sha256::digest(raw_key.as_bytes()).into();

    // Cache hit path
    {
        let mut guard = cache.lock().await;
        if let Some(cached) = guard.get(&sha256) {
            let cached = cached.clone();
            // Check expiry
            if let Some(exp) = cached.expires_at {
                if chrono::Utc::now().timestamp() > exp {
                    return Err(ApiKeyError::Expired);
                }
            }
            return Ok(cached);
        }
    }

    // Cache miss — extract prefix and query DB
    let prefix = &raw_key[5..17];
    let rows = db::api_keys::find_api_keys_by_prefix(pool, prefix).await?;

    if rows.is_empty() {
        return Err(ApiKeyError::NotFound);
    }

    // Argon2 verify against each candidate row (spawn_blocking because slow)
    let raw_key_owned = raw_key.to_string();
    let verified = tokio::task::spawn_blocking(move || {
        for row in &rows {
            if argon2_verify(&raw_key_owned, &row.key_hash) {
                return Some(CachedApiKey {
                    owner_email: row.owner_email.clone(),
                    key_prefix: row.key_prefix.clone(),
                    expires_at: row.expires_at,
                });
            }
        }
        None
    })
    .await
    .map_err(|e| ApiKeyError::HashError(e.to_string()))?;

    let entry = verified.ok_or(ApiKeyError::NotFound)?;

    // Check expiry
    if let Some(exp) = entry.expires_at {
        if chrono::Utc::now().timestamp() > exp {
            return Err(ApiKeyError::Expired);
        }
    }

    // Cache the verified key
    {
        let mut guard = cache.lock().await;
        guard.insert(sha256, entry.clone());
    }

    Ok(entry)
}

/// Generate random API key material: `(plaintext_key, prefix, argon2_hash)`.
///
/// Performs argon2 hashing in a blocking thread. The caller is responsible
/// for inserting into the database (via pool or transaction). This
/// separation ensures argon2 never runs while holding a DB transaction.
pub async fn generate_api_key_material() -> Result<(String, String, String), ApiKeyError> {
    let random_bytes: [u8; 32] = rand::thread_rng().r#gen();
    let hex_part = hex_encode(&random_bytes);
    let raw_key = format!("cbsk_{hex_part}");
    let prefix = raw_key[5..17].to_string();

    let key_clone = raw_key.clone();
    let hash = tokio::task::spawn_blocking(move || argon2_hash(&key_clone))
        .await
        .map_err(|e| ApiKeyError::HashError(e.to_string()))??;

    Ok((raw_key, prefix, hash))
}

/// Hash a raw API key with argon2id (default params).
fn argon2_hash(raw_key: &str) -> Result<String, ApiKeyError> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(raw_key.as_bytes(), &salt)
        .map_err(|e| ApiKeyError::HashError(e.to_string()))?;
    Ok(hash.to_string())
}

/// Verify a raw API key against an argon2 hash string.
fn argon2_verify(raw_key: &str, hash_str: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash_str) else {
        return false;
    };
    Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed)
        .is_ok()
}

/// Minimal hex encode (lowercase). Exposed for use by `db::seed`.
#[allow(dead_code)]
pub(crate) fn hex_encode_bytes(bytes: &[u8]) -> String {
    hex_encode(bytes)
}

/// Minimal hex encode (lowercase).
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
