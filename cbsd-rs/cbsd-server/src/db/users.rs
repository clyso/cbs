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
    pub default_channel_id: Option<i64>,
    pub is_robot: bool,
}

/// Error returned by [`create_or_update_user`].
///
/// `RobotNamePrefix` enforces the SSO forgery guard: a human account may
/// not be created or updated with a display name beginning with `robot:`
/// (the reserved prefix for service accounts). The OAuth callback maps
/// this to a 403 so the caller cannot spoof a service identity via the
/// Google display-name field.
#[derive(Debug)]
pub enum CreateOrUpdateUserError {
    RobotNamePrefix,
    Db(sqlx::Error),
}

impl std::fmt::Display for CreateOrUpdateUserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RobotNamePrefix => {
                f.write_str("display name starting with 'robot:' is reserved for service accounts")
            }
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for CreateOrUpdateUserError {}

impl From<sqlx::Error> for CreateOrUpdateUserError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// Create a new user or update the name of an existing user.
///
/// Rejects any `name` starting with the reserved `robot:` prefix; this is
/// the SSO forgery guard from design § Identity Model.
pub async fn create_or_update_user(
    pool: &SqlitePool,
    email: &str,
    name: &str,
) -> Result<UserRecord, CreateOrUpdateUserError> {
    if name.starts_with("robot:") {
        return Err(CreateOrUpdateUserError::RobotNamePrefix);
    }

    sqlx::query!(
        "INSERT INTO users (email, name) VALUES (?, ?)
         ON CONFLICT(email) DO UPDATE SET name = excluded.name, updated_at = unixepoch()",
        email,
        name,
    )
    .execute(pool)
    .await?;

    get_user(pool, email)
        .await?
        .ok_or(CreateOrUpdateUserError::Db(sqlx::Error::RowNotFound))
}

/// Get a user by email.
pub async fn get_user(pool: &SqlitePool, email: &str) -> Result<Option<UserRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT email AS "email!", name AS "name!", active, default_channel_id, is_robot
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
        is_robot: r.is_robot != 0,
    }))
}

/// Check if a user is active.
pub async fn is_user_active(pool: &SqlitePool, email: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!("SELECT active FROM users WHERE email = ?", email)
        .fetch_optional(pool)
        .await?;

    Ok(row.is_some_and(|r| r.active != 0))
}

/// Set a user's default channel. Pass `None` to clear.
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
            "file:users_test_{pid}_{id}?mode=memory&cache=shared",
            pid = std::process::id(),
        );
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .pragma("foreign_keys", "ON")
            .pragma("busy_timeout", "5000");
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn create_or_update_user_rejects_robot_prefix() {
        let pool = test_pool().await;
        let result = create_or_update_user(&pool, "alice@example.com", "robot:pretender").await;
        assert!(matches!(
            result,
            Err(CreateOrUpdateUserError::RobotNamePrefix)
        ));

        // Nothing was inserted.
        let row = sqlx::query!("SELECT COUNT(*) AS c FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.c, 0);
    }

    #[tokio::test]
    async fn create_or_update_user_allows_normal_names() {
        let pool = test_pool().await;
        let user = create_or_update_user(&pool, "alice@example.com", "Alice")
            .await
            .unwrap();
        assert_eq!(user.name, "Alice");
        assert!(!user.is_robot);
    }
}
