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

use sqlx::SqlitePool;

/// Insert a new token record. Returns the token row ID.
pub async fn insert_token(
    pool: &SqlitePool,
    user_email: &str,
    token_hash: &str,
    expires_at: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query!(
        "INSERT INTO tokens (user_email, token_hash, expires_at) VALUES (?, ?, ?)",
        user_email,
        token_hash,
        expires_at,
    )
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Check if a token is revoked (by its SHA-256 hash).
/// Returns true if revoked or unknown (unknown = treat as revoked).
pub async fn is_token_revoked(pool: &SqlitePool, token_hash: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT revoked FROM tokens WHERE token_hash = ?",
        token_hash,
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(r) => Ok(r.revoked != 0),
        None => Ok(true), // Unknown token = treat as revoked
    }
}

/// Revoke a single token by its SHA-256 hash. Returns true if a token was revoked.
pub async fn revoke_token(pool: &SqlitePool, token_hash: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE tokens SET revoked = 1 WHERE token_hash = ? AND revoked = 0",
        token_hash,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Revoke all tokens for a given user. Returns number of tokens revoked.
pub async fn revoke_all_for_user(pool: &SqlitePool, user_email: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE tokens SET revoked = 1 WHERE user_email = ? AND revoked = 0",
        user_email,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Record successful use of a PASETO token. Sets `first_used_at` once (if
/// NULL) and always updates `last_used_at`. Fire-and-forget — callers
/// should log errors and proceed rather than failing the request.
pub async fn mark_token_used(pool: &SqlitePool, token_hash: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE tokens
         SET first_used_at = COALESCE(first_used_at, unixepoch()),
             last_used_at  = unixepoch()
         WHERE token_hash = ? AND revoked = 0",
        token_hash,
    )
    .execute(pool)
    .await?;
    Ok(())
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
            "file:tokens_test_{pid}_{id}?mode=memory&cache=shared",
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
    async fn mark_token_used_preserves_first_used_at_across_calls() {
        let pool = test_pool().await;
        seed_user(&pool, "alice@example.com").await;
        insert_token(&pool, "alice@example.com", "hash-a", None)
            .await
            .unwrap();

        mark_token_used(&pool, "hash-a").await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM tokens WHERE token_hash = ?",
            "hash-a",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let first_at = row.first_used_at.expect("first_used_at set on first call");
        assert!(row.last_used_at.is_some(), "last_used_at set on first call");

        // Simulate a later use by zeroing last_used_at before the second call.
        sqlx::query!(
            "UPDATE tokens SET last_used_at = 0 WHERE token_hash = ?",
            "hash-a",
        )
        .execute(&pool)
        .await
        .unwrap();

        mark_token_used(&pool, "hash-a").await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM tokens WHERE token_hash = ?",
            "hash-a",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.first_used_at,
            Some(first_at),
            "first_used_at preserved by COALESCE on subsequent calls"
        );
        assert_ne!(
            row.last_used_at,
            Some(0),
            "last_used_at overwritten on each call"
        );
    }

    #[tokio::test]
    async fn mark_token_used_skips_revoked_rows() {
        let pool = test_pool().await;
        seed_user(&pool, "bob@example.com").await;
        insert_token(&pool, "bob@example.com", "hash-b", None)
            .await
            .unwrap();
        revoke_token(&pool, "hash-b").await.unwrap();

        mark_token_used(&pool, "hash-b").await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM tokens WHERE token_hash = ?",
            "hash-b",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            row.first_used_at.is_none(),
            "revoked token row is not touched"
        );
        assert!(
            row.last_used_at.is_none(),
            "revoked token row is not touched"
        );
    }
}
