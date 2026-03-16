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

//! Database operations for API keys.

use sqlx::{Row, SqlitePool};

/// Full API key row (including hash) for verification.
#[allow(dead_code)]
pub struct ApiKeyRow {
    pub id: i64,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub owner_email: String,
    pub expires_at: Option<i64>,
    pub revoked: bool,
    pub created_at: i64,
}

/// Summary of an API key for listing (no hash exposed).
pub struct ApiKeyListItem {
    pub key_prefix: String,
    pub name: String,
    pub created_at: i64,
}

/// Insert a new API key. Returns the row ID.
pub async fn insert_api_key(
    pool: &SqlitePool,
    name: &str,
    owner_email: &str,
    key_hash: &str,
    key_prefix: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO api_keys (name, key_hash, key_prefix, owner_email) VALUES (?, ?, ?, ?)",
    )
    .bind(name)
    .bind(key_hash)
    .bind(key_prefix)
    .bind(owner_email)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Find all non-revoked API keys with the given prefix (across all owners).
/// Used for verification: we iterate results and argon2-verify against each.
pub async fn find_api_keys_by_prefix(
    pool: &SqlitePool,
    key_prefix: &str,
) -> Result<Vec<ApiKeyRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, key_hash, key_prefix, owner_email, expires_at, revoked, created_at
         FROM api_keys
         WHERE key_prefix = ? AND revoked = 0",
    )
    .bind(key_prefix)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ApiKeyRow {
            id: r.get("id"),
            name: r.get("name"),
            key_hash: r.get("key_hash"),
            key_prefix: r.get("key_prefix"),
            owner_email: r.get("owner_email"),
            expires_at: r.get("expires_at"),
            revoked: r.get::<i32, _>("revoked") != 0,
            created_at: r.get("created_at"),
        })
        .collect())
}

/// List all API keys for a user (prefix + name + created_at, no hash).
pub async fn list_api_keys_for_user(
    pool: &SqlitePool,
    owner_email: &str,
) -> Result<Vec<ApiKeyListItem>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT key_prefix, name, created_at
         FROM api_keys
         WHERE owner_email = ? AND revoked = 0
         ORDER BY created_at DESC",
    )
    .bind(owner_email)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ApiKeyListItem {
            key_prefix: r.get("key_prefix"),
            name: r.get("name"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Revoke an API key by owner + prefix. Returns true if a key was revoked.
pub async fn revoke_api_key_by_prefix(
    pool: &SqlitePool,
    owner_email: &str,
    key_prefix: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE api_keys SET revoked = 1
         WHERE owner_email = ? AND key_prefix = ? AND revoked = 0",
    )
    .bind(owner_email)
    .bind(key_prefix)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Revoke all API keys for a user. Returns number of keys revoked.
pub async fn revoke_all_api_keys_for_user(
    pool: &SqlitePool,
    owner_email: &str,
) -> Result<u64, sqlx::Error> {
    let result =
        sqlx::query("UPDATE api_keys SET revoked = 1 WHERE owner_email = ? AND revoked = 0")
            .bind(owner_email)
            .execute(pool)
            .await?;

    Ok(result.rows_affected())
}

/// Revoke an API key by its row ID. No owner filter — used by worker
/// deregistration and token regeneration where the calling admin may not
/// be the original key creator. Returns true if a key was revoked.
#[allow(dead_code)]
pub async fn revoke_api_key_by_id(pool: &SqlitePool, api_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = ? AND revoked = 0")
        .bind(api_key_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Insert an API key inside an existing transaction. Returns the row ID
/// via `last_insert_rowid()`.
pub async fn insert_api_key_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    owner_email: &str,
    key_hash: &str,
    key_prefix: &str,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO api_keys (name, key_hash, key_prefix, owner_email) VALUES (?, ?, ?, ?)",
    )
    .bind(name)
    .bind(key_hash)
    .bind(key_prefix)
    .bind(owner_email)
    .execute(&mut **tx)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Get the key prefix for an API key by its row ID. Used to purge the
/// LRU cache after revocation when only the row ID is known.
pub async fn get_key_prefix_by_id(
    pool: &SqlitePool,
    api_key_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT key_prefix FROM api_keys WHERE id = ?")
        .bind(api_key_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| r.get("key_prefix")))
}
