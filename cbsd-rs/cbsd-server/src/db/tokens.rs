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

use sqlx::{Row, SqlitePool};

/// Insert a new token record. Returns the token row ID.
pub async fn insert_token(
    pool: &SqlitePool,
    user_email: &str,
    token_hash: &str,
    expires_at: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO tokens (user_email, token_hash, expires_at) VALUES (?, ?, ?)",
    )
    .bind(user_email)
    .bind(token_hash)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Check if a token is revoked (by its SHA-256 hash).
/// Returns true if revoked or unknown (unknown = treat as revoked).
pub async fn is_token_revoked(pool: &SqlitePool, token_hash: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT revoked FROM tokens WHERE token_hash = ?")
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;

    match row {
        Some(r) => Ok(r.get::<i32, _>("revoked") != 0),
        None => Ok(true), // Unknown token = treat as revoked
    }
}

/// Revoke a single token by its SHA-256 hash. Returns true if a token was revoked.
pub async fn revoke_token(pool: &SqlitePool, token_hash: &str) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("UPDATE tokens SET revoked = 1 WHERE token_hash = ? AND revoked = 0")
            .bind(token_hash)
            .execute(pool)
            .await?;

    Ok(result.rows_affected() > 0)
}

/// Revoke all tokens for a given user. Returns number of tokens revoked.
pub async fn revoke_all_for_user(pool: &SqlitePool, user_email: &str) -> Result<u64, sqlx::Error> {
    let result =
        sqlx::query("UPDATE tokens SET revoked = 1 WHERE user_email = ? AND revoked = 0")
            .bind(user_email)
            .execute(pool)
            .await?;

    Ok(result.rows_affected())
}
