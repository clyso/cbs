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

pub struct UserRecord {
    pub email: String,
    pub name: String,
    pub active: bool,
    #[allow(dead_code)]
    pub default_channel_id: Option<i64>,
}

/// Create a new user or update the name of an existing user.
pub async fn create_or_update_user(
    pool: &SqlitePool,
    email: &str,
    name: &str,
) -> Result<UserRecord, sqlx::Error> {
    sqlx::query!(
        "INSERT INTO users (email, name) VALUES (?, ?)
         ON CONFLICT(email) DO UPDATE SET name = excluded.name, updated_at = unixepoch()",
        email,
        name,
    )
    .execute(pool)
    .await?;

    get_user(pool, email).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Get a user by email.
pub async fn get_user(pool: &SqlitePool, email: &str) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT email AS "email!", name AS "name!", active, default_channel_id
         FROM users WHERE email = ?"#,
        email,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| UserRecord {
        email: r.email,
        name: r.name,
        active: r.active != 0,
        default_channel_id: r.default_channel_id,
    }))
}

/// Check if a user is active.
#[allow(dead_code)]
pub async fn is_user_active(pool: &SqlitePool, email: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!("SELECT active FROM users WHERE email = ?", email)
        .fetch_optional(pool)
        .await?;

    Ok(row.is_some_and(|r| r.active != 0))
}

/// Set a user's default channel. Pass `None` to clear.
#[allow(dead_code)]
pub async fn set_default_channel(
    pool: &SqlitePool,
    email: &str,
    channel_id: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE users SET default_channel_id = ?, updated_at = unixepoch()
         WHERE email = ?",
        channel_id,
        email,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}
