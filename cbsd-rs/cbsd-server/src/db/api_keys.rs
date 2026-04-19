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

use sqlx::SqlitePool;

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
    let result = sqlx::query!(
        "INSERT INTO api_keys (name, key_hash, key_prefix, owner_email) VALUES (?, ?, ?, ?)",
        name,
        key_hash,
        key_prefix,
        owner_email,
    )
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
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", name AS "name!", key_hash AS "key_hash!", key_prefix AS "key_prefix!",
                  owner_email AS "owner_email!", expires_at, revoked AS "revoked!", created_at AS "created_at!"
           FROM api_keys
           WHERE key_prefix = ? AND revoked = 0"#,
        key_prefix,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ApiKeyRow {
            id: r.id,
            name: r.name,
            key_hash: r.key_hash,
            key_prefix: r.key_prefix,
            owner_email: r.owner_email,
            expires_at: r.expires_at,
            revoked: r.revoked != 0,
            created_at: r.created_at,
        })
        .collect())
}

/// List all API keys for a user (prefix + name + created_at, no hash).
pub async fn list_api_keys_for_user(
    pool: &SqlitePool,
    owner_email: &str,
) -> Result<Vec<ApiKeyListItem>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT key_prefix AS "key_prefix!", name AS "name!", created_at AS "created_at!"
           FROM api_keys
           WHERE owner_email = ? AND revoked = 0
           ORDER BY created_at DESC"#,
        owner_email,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ApiKeyListItem {
            key_prefix: r.key_prefix,
            name: r.name,
            created_at: r.created_at,
        })
        .collect())
}

/// Revoke an API key by owner + prefix. Returns true if a key was revoked.
pub async fn revoke_api_key_by_prefix(
    pool: &SqlitePool,
    owner_email: &str,
    key_prefix: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE api_keys SET revoked = 1
         WHERE owner_email = ? AND key_prefix = ? AND revoked = 0",
        owner_email,
        key_prefix,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Revoke all API keys for a user. Returns number of keys revoked.
pub async fn revoke_all_api_keys_for_user(
    pool: &SqlitePool,
    owner_email: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE api_keys SET revoked = 1 WHERE owner_email = ? AND revoked = 0",
        owner_email,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Revoke an API key by its row ID. No owner filter — used by worker
/// deregistration and token regeneration where the calling admin may not
/// be the original key creator. Returns true if a key was revoked.
#[allow(dead_code)]
pub async fn revoke_api_key_by_id(pool: &SqlitePool, api_key_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE api_keys SET revoked = 1 WHERE id = ? AND revoked = 0",
        api_key_id,
    )
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
    let result = sqlx::query!(
        "INSERT INTO api_keys (name, key_hash, key_prefix, owner_email) VALUES (?, ?, ?, ?)",
        name,
        key_hash,
        key_prefix,
        owner_email,
    )
    .execute(&mut **tx)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Record successful use of an API key. Sets `first_used_at` once (if
/// NULL) and always updates `last_used_at`. Fire-and-forget — callers
/// should log errors and proceed rather than failing the request.
pub async fn mark_api_key_used(pool: &SqlitePool, api_key_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE api_keys
         SET first_used_at = COALESCE(first_used_at, unixepoch()),
             last_used_at  = unixepoch()
         WHERE id = ? AND revoked = 0",
        api_key_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the key prefix for an API key by its row ID. Used to purge the
/// LRU cache after revocation when only the row ID is known.
pub async fn get_key_prefix_by_id(
    pool: &SqlitePool,
    api_key_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT key_prefix AS "key_prefix!" FROM api_keys WHERE id = ?"#,
        api_key_id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.key_prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn test_pool() -> SqlitePool {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let url = format!(
            "file:api_keys_test_{pid}_{id}?mode=memory&cache=shared",
            pid = std::process::id(),
        );
        let options = SqliteConnectOptions::from_str(&url)
            .expect("valid sqlite URL")
            .pragma("foreign_keys", "ON")
            .pragma("busy_timeout", "5000");
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .connect_with(options)
            .await
            .expect("pool");
        sqlx::migrate!("../migrations")
            .run(&pool)
            .await
            .expect("migrations");
        pool
    }

    async fn seed_user(pool: &SqlitePool, email: &str) {
        sqlx::query!(
            "INSERT INTO users (email, name, active, is_robot) VALUES (?, ?, 1, 0)",
            email,
            email,
        )
        .execute(pool)
        .await
        .expect("seed user");
    }

    #[tokio::test]
    async fn mark_api_key_used_preserves_first_used_at_across_calls() {
        let pool = test_pool().await;
        seed_user(&pool, "alice@example.com").await;
        let key_id = insert_api_key(&pool, "ci", "alice@example.com", "hash-a", "pfx000000aaa")
            .await
            .unwrap();

        mark_api_key_used(&pool, key_id).await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM api_keys WHERE id = ?",
            key_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let first_at = row.first_used_at.expect("first_used_at set on first call");
        assert!(row.last_used_at.is_some());

        sqlx::query!("UPDATE api_keys SET last_used_at = 0 WHERE id = ?", key_id)
            .execute(&pool)
            .await
            .unwrap();

        mark_api_key_used(&pool, key_id).await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM api_keys WHERE id = ?",
            key_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.first_used_at, Some(first_at));
        assert_ne!(row.last_used_at, Some(0));
    }

    #[tokio::test]
    async fn mark_api_key_used_skips_revoked_rows() {
        let pool = test_pool().await;
        seed_user(&pool, "bob@example.com").await;
        let key_id = insert_api_key(&pool, "ci", "bob@example.com", "hash-b", "pfx000000bbb")
            .await
            .unwrap();
        sqlx::query!("UPDATE api_keys SET revoked = 1 WHERE id = ?", key_id)
            .execute(&pool)
            .await
            .unwrap();

        mark_api_key_used(&pool, key_id).await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM api_keys WHERE id = ?",
            key_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.first_used_at.is_none());
        assert!(row.last_used_at.is_none());
    }
}
