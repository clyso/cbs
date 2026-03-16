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

pub struct UserRecord {
    pub email: String,
    pub name: String,
    pub active: bool,
}

/// Create a new user or update the name of an existing user.
pub async fn create_or_update_user(
    pool: &SqlitePool,
    email: &str,
    name: &str,
) -> Result<UserRecord, sqlx::Error> {
    sqlx::query(
        "INSERT INTO users (email, name) VALUES (?, ?)
         ON CONFLICT(email) DO UPDATE SET name = excluded.name, updated_at = unixepoch()",
    )
    .bind(email)
    .bind(name)
    .execute(pool)
    .await?;

    get_user(pool, email).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Get a user by email.
pub async fn get_user(pool: &SqlitePool, email: &str) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query("SELECT email, name, active FROM users WHERE email = ?")
        .bind(email)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| UserRecord {
        email: r.get("email"),
        name: r.get("name"),
        active: r.get::<i32, _>("active") != 0,
    }))
}

/// Check if a user is active.
#[allow(dead_code)]
pub async fn is_user_active(pool: &SqlitePool, email: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT active FROM users WHERE email = ?")
        .bind(email)
        .fetch_optional(pool)
        .await?;

    Ok(row.is_some_and(|r| r.get::<i32, _>("active") != 0))
}
