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

use sqlx::{SqliteConnection, SqlitePool};

#[derive(Debug)]
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
        "INSERT INTO users (email, name, first_login_at) VALUES (?, ?, unixepoch())
         ON CONFLICT(email) DO UPDATE SET
             name = excluded.name,
             updated_at = unixepoch(),
             first_login_at = COALESCE(users.first_login_at, unixepoch())",
        email,
        name,
    )
    .execute(pool)
    .await?;

    get_user(pool, email)
        .await?
        .ok_or(CreateOrUpdateUserError::Db(sqlx::Error::RowNotFound))
}

/// Error returned by [`provision_user`]. Each variant maps to a distinct HTTP
/// status at the route layer (design 020).
#[derive(Debug)]
pub enum ProvisionUserError {
    /// A human row already exists for this email (active or deactivated).
    AlreadyExists,
    /// The email is in the robot namespace (`robot+…@robots`) or an
    /// `is_robot = 1` row holds it.
    RobotCollision,
    /// `name` begins with the reserved `robot:` prefix.
    RobotNamePrefix,
    /// A requested role does not exist.
    UnknownRole(String),
    /// A UNIQUE constraint fired (a concurrent insert committed first).
    UniqueViolation,
    /// Any other database error.
    Db(sqlx::Error),
}

impl std::fmt::Display for ProvisionUserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyExists => f.write_str("a user already exists for this email"),
            Self::RobotCollision => f.write_str("email is reserved for service accounts"),
            Self::RobotNamePrefix => {
                f.write_str("display name starting with 'robot:' is reserved for service accounts")
            }
            Self::UnknownRole(r) => write!(f, "role '{r}' does not exist"),
            Self::UniqueViolation => f.write_str("concurrent modification detected"),
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for ProvisionUserError {}

impl From<sqlx::Error> for ProvisionUserError {
    fn from(e: sqlx::Error) -> Self {
        if super::is_unique_violation(&e) {
            Self::UniqueViolation
        } else {
            Self::Db(e)
        }
    }
}

/// Provision (pre-create) a human user with zero or more roles before they
/// have ever logged in, so the roles are in effect on first login. `email` is
/// assumed already normalized (lowercase) by the caller. Returns the created
/// `UserRecord`.
///
/// Runs under `BEGIN IMMEDIATE` and re-reads the row under the write lock so
/// two concurrent creates serialize: the loser observes the winner's row and
/// returns `AlreadyExists` rather than a raw 500 (design 020). A deactivated
/// human is **not** revived — the remediation is `activate` + the role
/// endpoints.
pub async fn provision_user(
    pool: &SqlitePool,
    email: &str,
    name: &str,
    roles: &[&str],
) -> Result<UserRecord, ProvisionUserError> {
    if name.starts_with("robot:") {
        return Err(ProvisionUserError::RobotNamePrefix);
    }
    if super::robots::synthetic_email_to_name(email).is_some() {
        return Err(ProvisionUserError::RobotCollision);
    }

    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    match provision_user_inner(&mut conn, email, name, roles).await {
        Ok(()) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(e);
        }
    }

    get_user(pool, email)
        .await?
        .ok_or(ProvisionUserError::Db(sqlx::Error::RowNotFound))
}

async fn provision_user_inner(
    conn: &mut SqliteConnection,
    email: &str,
    name: &str,
    roles: &[&str],
) -> Result<(), ProvisionUserError> {
    let existing = sqlx::query!(
        r#"SELECT is_robot AS "is_robot!" FROM users WHERE email = ?"#,
        email,
    )
    .fetch_optional(&mut *conn)
    .await?;
    match existing {
        Some(r) if r.is_robot != 0 => return Err(ProvisionUserError::RobotCollision),
        Some(_) => return Err(ProvisionUserError::AlreadyExists),
        None => {}
    }

    // Validate roles inside the transaction so the check and the insert are
    // atomic and a missing role yields a clear error rather than an opaque FK
    // failure.
    for role in roles {
        let exists = sqlx::query!("SELECT name FROM roles WHERE name = ?", role)
            .fetch_optional(&mut *conn)
            .await?
            .is_some();
        if !exists {
            return Err(ProvisionUserError::UnknownRole((*role).to_string()));
        }
    }

    // A provisioned user has first_login_at = NULL (the "pending" state) until
    // they log in; active defaults to 1 and is_robot to 0.
    sqlx::query!("INSERT INTO users (email, name) VALUES (?, ?)", email, name)
        .execute(&mut *conn)
        .await?;

    for role in roles {
        sqlx::query!(
            "INSERT OR IGNORE INTO user_roles (user_email, role_name) VALUES (?, ?)",
            email,
            *role,
        )
        .execute(&mut *conn)
        .await?;
    }

    Ok(())
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
    /// Unix epoch seconds of first login; `None` for a provisioned user who
    /// has never logged in (the "pending" state). Always `None` for robots.
    pub first_login_at: Option<i64>,
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
                    first_login_at: r.first_login_at,
                })
                .collect::<Vec<_>>()
        };
    }

    let result = match filter {
        EntityFilter::User => {
            let rows = sqlx::query!(
                r#"SELECT email AS "email!", name AS "name!", active AS "active!",
                          is_robot AS "is_robot!", first_login_at
                   FROM users WHERE is_robot = 0 ORDER BY email"#,
            )
            .fetch_all(pool)
            .await?;
            to_summaries!(rows)
        }
        EntityFilter::Robot => {
            let rows = sqlx::query!(
                r#"SELECT email AS "email!", name AS "name!", active AS "active!",
                          is_robot AS "is_robot!", first_login_at
                   FROM users WHERE is_robot = 1 ORDER BY email"#,
            )
            .fetch_all(pool)
            .await?;
            to_summaries!(rows)
        }
        EntityFilter::All => {
            let rows = sqlx::query!(
                r#"SELECT email AS "email!", name AS "name!", active AS "active!",
                          is_robot AS "is_robot!", first_login_at
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
    async fn create_or_update_user_stamps_and_preserves_first_login_at() {
        let pool = test_pool().await;
        create_or_update_user(&pool, "alice@example.com", "Alice")
            .await
            .unwrap();
        let first: Option<i64> = sqlx::query_scalar(
            "SELECT first_login_at FROM users WHERE email = 'alice@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(first.is_some(), "first login must stamp first_login_at");

        // Pin first_login_at to a fixed past value, then log in again: the
        // upsert must PRESERVE it via COALESCE, not overwrite with unixepoch().
        // (unixepoch() is whole-second, so without a fixed pin a same-second
        // re-login could not distinguish "preserved" from "re-stamped".)
        sqlx::query(
            "UPDATE users SET first_login_at = 1000000000 WHERE email = 'alice@example.com'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let user = create_or_update_user(&pool, "alice@example.com", "Alice Renamed")
            .await
            .unwrap();
        assert_eq!(
            user.name, "Alice Renamed",
            "subsequent login refreshes the name"
        );

        let preserved: Option<i64> = sqlx::query_scalar(
            "SELECT first_login_at FROM users WHERE email = 'alice@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            preserved,
            Some(1_000_000_000),
            "first_login_at must be preserved across logins, not overwritten"
        );
    }

    async fn seed_role(pool: &SqlitePool, name: &str) {
        sqlx::query("INSERT INTO roles (name, description, builtin) VALUES (?, '', 0)")
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn provision_user_creates_pending_user_with_roles() {
        let pool = test_pool().await;
        seed_role(&pool, "builder").await;
        let user = provision_user(&pool, "alice@example.com", "Alice", &["builder"])
            .await
            .unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert!(!user.is_robot);
        assert!(user.active);

        // A provisioned user is pending: no first_login_at until they log in.
        let first: Option<i64> = sqlx::query_scalar(
            "SELECT first_login_at FROM users WHERE email = 'alice@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            first.is_none(),
            "a provisioned user is pending (NULL first_login_at)"
        );

        let role_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_roles \
             WHERE user_email = 'alice@example.com' AND role_name = 'builder'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role_count, 1, "the provisioned role must be assigned");
    }

    #[tokio::test]
    async fn provision_then_login_preserves_roles_and_stamps_first_login() {
        let pool = test_pool().await;
        seed_role(&pool, "builder").await;
        provision_user(&pool, "alice@example.com", "Alice", &["builder"])
            .await
            .unwrap();

        // First login refreshes the name and stamps first_login_at; the
        // pre-assigned role persists.
        let user = create_or_update_user(&pool, "alice@example.com", "Alice Real")
            .await
            .unwrap();
        assert_eq!(user.name, "Alice Real");

        let first: Option<i64> = sqlx::query_scalar(
            "SELECT first_login_at FROM users WHERE email = 'alice@example.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(first.is_some(), "first login must stamp first_login_at");

        let role_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_roles \
             WHERE user_email = 'alice@example.com' AND role_name = 'builder'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            role_count, 1,
            "the pre-assigned role must survive first login"
        );
    }

    #[tokio::test]
    async fn provision_user_rejects_duplicate() {
        let pool = test_pool().await;
        provision_user(&pool, "alice@example.com", "Alice", &[])
            .await
            .unwrap();
        let err = provision_user(&pool, "alice@example.com", "Alice", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProvisionUserError::AlreadyExists));
    }

    #[tokio::test]
    async fn provision_user_rejects_unknown_role_and_rolls_back() {
        let pool = test_pool().await;
        let err = provision_user(&pool, "alice@example.com", "Alice", &["ghost"])
            .await
            .unwrap_err();
        assert!(matches!(err, ProvisionUserError::UnknownRole(r) if r == "ghost"));

        // The whole provision rolled back: no user row was created.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "an unknown role must roll back the provision");
    }

    #[tokio::test]
    async fn provision_user_rejects_robot_name_and_email() {
        let pool = test_pool().await;
        let err = provision_user(&pool, "alice@example.com", "robot:pretender", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProvisionUserError::RobotNamePrefix));

        let err = provision_user(&pool, "robot+ci@robots", "CI", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, ProvisionUserError::RobotCollision));
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
