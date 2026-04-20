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

/// Filter for `list_entities_filtered`.
pub enum EntityFilter {
    User,
    Robot,
    All,
}

/// A summary row returned by `list_entities_filtered`.
pub struct EntitySummary {
    pub email: String,
    pub name: String,
    pub active: bool,
    pub is_robot: bool,
}

/// List entities filtered by kind. Uses three separate compile-time checked
/// queries because `sqlx::query!` does not support dynamic WHERE clauses.
pub async fn list_entities_filtered(
    pool: &SqlitePool,
    filter: EntityFilter,
) -> Result<Vec<EntitySummary>, sqlx::Error> {
    macro_rules! to_summaries {
        ($rows:expr) => {
            $rows
                .into_iter()
                .map(|r| EntitySummary {
                    email: r.email,
                    name: r.name,
                    active: r.active != 0,
                    is_robot: r.is_robot != 0,
                })
                .collect::<Vec<_>>()
        };
    }

    let result = match filter {
        EntityFilter::User => {
            let rows = sqlx::query!(
                r#"SELECT email AS "email!", name AS "name!", active AS "active!",
                          is_robot AS "is_robot!"
                   FROM users WHERE is_robot = 0 ORDER BY email"#,
            )
            .fetch_all(pool)
            .await?;
            to_summaries!(rows)
        }
        EntityFilter::Robot => {
            let rows = sqlx::query!(
                r#"SELECT email AS "email!", name AS "name!", active AS "active!",
                          is_robot AS "is_robot!"
                   FROM users WHERE is_robot = 1 ORDER BY email"#,
            )
            .fetch_all(pool)
            .await?;
            to_summaries!(rows)
        }
        EntityFilter::All => {
            let rows = sqlx::query!(
                r#"SELECT email AS "email!", name AS "name!", active AS "active!",
                          is_robot AS "is_robot!"
                   FROM users ORDER BY email"#,
            )
            .fetch_all(pool)
            .await?;
            to_summaries!(rows)
        }
    };

    Ok(result)
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

    #[tokio::test]
    async fn list_entities_filtered_respects_type_filter() {
        let pool = test_pool().await;

        // Seed one human and one robot.
        create_or_update_user(&pool, "alice@example.com", "Alice")
            .await
            .unwrap();
        sqlx::query!(
            "INSERT INTO users (email, name, is_robot) VALUES ('robot+ci@robots', 'robot:ci', 1)"
        )
        .execute(&pool)
        .await
        .unwrap();

        // type=user → one human, no robot.
        let users = list_entities_filtered(&pool, EntityFilter::User)
            .await
            .unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].email, "alice@example.com");
        assert!(!users[0].is_robot);

        // type=robot → one robot, no human.
        let robots = list_entities_filtered(&pool, EntityFilter::Robot)
            .await
            .unwrap();
        assert_eq!(robots.len(), 1);
        assert_eq!(robots[0].email, "robot+ci@robots");
        assert!(robots[0].is_robot);

        // type=all → both, sorted by email.
        let all = list_entities_filtered(&pool, EntityFilter::All)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        // "alice@example.com" sorts before "robot+ci@robots" lexically.
        assert_eq!(all[0].email, "alice@example.com");
        assert_eq!(all[1].email, "robot+ci@robots");
    }
}
