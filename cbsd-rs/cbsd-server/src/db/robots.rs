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

//! Database operations for robot accounts and robot tokens.

use sqlx::{SqliteConnection, SqlitePool};

/// A non-revoked robot token candidate returned by prefix lookup.
/// The caller must do Argon2id verification on the hash field.
pub struct TokenCandidate {
    pub id: i64,
    pub token_hash: String,
    pub token_prefix: String,
    pub robot_email: String,
    pub expires_at: Option<i64>,
}

/// A robot account row as stored in the database, enriched with its
/// current "best" token (non-revoked if any, otherwise the most recent
/// revoked row). Populated by the same CTE used by both `list_robots` and
/// `get_robot_by_name` so the list and detail views never disagree about
/// which token row represents a robot's current state.
pub struct RobotRow {
    pub email: String,
    /// The display identity stored as `users.name` — e.g. `robot:ci`.
    pub display_name: String,
    pub description: Option<String>,
    pub active: bool,
    pub created_at: i64,
    /// `"active" | "expired" | "revoked" | "none"` (per design § REST API
    /// "Get Robot"). `"none"` when no token row has ever existed;
    /// `"revoked"` when every existing row is a tombstone.
    pub token_state: String,
    pub token_prefix: Option<String>,
    pub token_expires_at: Option<i64>,
    pub token_first_used_at: Option<i64>,
    pub token_last_used_at: Option<i64>,
    pub token_created_at: Option<i64>,
}

/// Outcome of `create_or_revive`: whether a fresh row was inserted or a
/// tombstoned row was reactivated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateRevivedOutcome {
    Created,
    Revived,
}

/// Error surface of `create_or_revive`. Each variant maps to a distinct
/// HTTP status code at the route layer.
#[derive(Debug)]
pub enum CreateRobotError {
    /// A row exists with `is_robot = 1` and `active = 1`; the caller cannot
    /// create a new robot under this name.
    AlreadyActive,
    /// A row exists with `is_robot = 0` — a human user already holds the
    /// synthetic email; refuse the create.
    HumanCollision,
    /// A SQLite UNIQUE constraint fired (partial unique index on an
    /// active token, or a concurrent insert that committed first). Maps
    /// to 409 so the loser of a race never surfaces a raw 500.
    UniqueViolation,
    /// Any other database error.
    Db(sqlx::Error),
}

impl std::fmt::Display for CreateRobotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyActive => f.write_str("robot already exists and is active"),
            Self::HumanCollision => f.write_str("name collides with a human account"),
            Self::UniqueViolation => f.write_str("concurrent modification detected"),
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for CreateRobotError {}

impl From<sqlx::Error> for CreateRobotError {
    fn from(e: sqlx::Error) -> Self {
        if super::is_unique_violation(&e) {
            Self::UniqueViolation
        } else {
            Self::Db(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Name ↔ email helpers
// ---------------------------------------------------------------------------

pub fn name_to_synthetic_email(name: &str) -> String {
    format!("robot+{name}@robots")
}

pub fn synthetic_email_to_name(email: &str) -> Option<&str> {
    email.strip_prefix("robot+")?.strip_suffix("@robots")
}

/// Validate a robot name: `^[a-zA-Z0-9_][a-zA-Z0-9_.-]*[a-zA-Z0-9_]$`
/// with total length 2–64 characters. The length check uses `chars().count()`
/// rather than `str::len()` so a multi-byte non-ASCII input (e.g. "робот")
/// is classified by character count and then falls through to the
/// character-class check, rather than indexing past the char slice.
pub fn validate_robot_name(name: &str) -> Result<(), String> {
    let chars: Vec<char> = name.chars().collect();
    let n = chars.len();
    if !(2..=64).contains(&n) {
        return Err(format!("robot name must be 2–64 characters, got {n}"));
    }
    let first = chars[0];
    let last = chars[n - 1];

    let is_border = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if !is_border(first) {
        return Err(format!(
            "robot name must start with [a-zA-Z0-9_], got '{first}'"
        ));
    }
    if !is_border(last) {
        return Err(format!(
            "robot name must end with [a-zA-Z0-9_], got '{last}'"
        ));
    }
    for c in &chars[1..n - 1] {
        if !c.is_ascii_alphanumeric() && *c != '_' && *c != '.' && *c != '-' {
            return Err(format!(
                "robot name contains invalid character '{c}': allowed [a-zA-Z0-9_.-]"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Token lookups (called by token_cache::verify_robot_token)
// ---------------------------------------------------------------------------

/// Return all non-revoked robot tokens matching the given prefix.
/// The caller performs Argon2id hash verification on the results.
pub async fn find_active_token_by_prefix(
    pool: &SqlitePool,
    prefix: &str,
) -> Result<Vec<TokenCandidate>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", token_hash AS "token_hash!",
                  token_prefix AS "token_prefix!", robot_email AS "robot_email!",
                  expires_at
           FROM robot_tokens
           WHERE token_prefix = ? AND revoked = 0"#,
        prefix,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TokenCandidate {
            id: r.id,
            token_hash: r.token_hash,
            token_prefix: r.token_prefix,
            robot_email: r.robot_email,
            expires_at: r.expires_at,
        })
        .collect())
}

/// Mark every non-revoked row for this robot as revoked on an open
/// connection. Returns the number of rows affected. Callers that run
/// under `BEGIN IMMEDIATE` use this form; a `_pool` variant wraps it for
/// callers that don't need an enclosing transaction.
pub async fn revoke_all_active_tokens_in_conn(
    conn: &mut SqliteConnection,
    email: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE robot_tokens SET revoked = 1 WHERE robot_email = ? AND revoked = 0",
        email,
    )
    .execute(&mut *conn)
    .await?;
    Ok(result.rows_affected())
}

/// Standalone revoke: acquire a connection, revoke every non-revoked
/// token for the robot, commit. Unlike tombstone, leaves `users.active`
/// unchanged so the admin can re-issue a token without a revive cycle.
pub async fn revoke_all_active_tokens(pool: &SqlitePool, email: &str) -> Result<u64, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    revoke_all_active_tokens_in_conn(&mut conn, email).await
}

/// Fire-and-forget: update `last_used_at` and set `first_used_at` once.
pub async fn mark_robot_token_used(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE robot_tokens
         SET last_used_at = unixepoch(),
             first_used_at = COALESCE(first_used_at, unixepoch())
         WHERE id = ?",
        id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Robot CRUD
// ---------------------------------------------------------------------------

/// List all robot accounts (active and tombstoned) with token status.
pub async fn list_robots(pool: &SqlitePool) -> Result<Vec<RobotRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"WITH best_token AS (
               SELECT robot_email, revoked, token_prefix, expires_at,
                      first_used_at, last_used_at,
                      created_at AS token_created_at,
                      ROW_NUMBER() OVER (PARTITION BY robot_email
                                         ORDER BY revoked ASC, id DESC) AS rn
               FROM robot_tokens
           )
           SELECT u.email              AS "email!",
                  u.name               AS "display_name!",
                  u.robot_description,
                  u.active             AS "active!",
                  u.created_at         AS "created_at!",
                  bt.revoked           AS "token_revoked?: i64",
                  bt.token_prefix      AS "token_prefix?",
                  bt.expires_at        AS "token_expires_at?",
                  bt.first_used_at     AS "token_first_used_at?",
                  bt.last_used_at      AS "token_last_used_at?",
                  bt.token_created_at  AS "token_created_at?"
           FROM users u
           LEFT JOIN best_token bt
             ON bt.robot_email = u.email AND bt.rn = 1
           WHERE u.is_robot = 1
           ORDER BY u.email"#,
    )
    .fetch_all(pool)
    .await?;

    let now = chrono::Utc::now().timestamp();
    Ok(rows
        .into_iter()
        .map(|r| {
            classify_robot_row(
                r.email,
                r.display_name,
                r.robot_description,
                r.active != 0,
                r.created_at,
                r.token_revoked,
                r.token_prefix,
                r.token_expires_at,
                r.token_first_used_at,
                r.token_last_used_at,
                r.token_created_at,
                now,
            )
        })
        .collect())
}

/// Fetch a single robot account by name. Returns `None` if not found.
pub async fn get_robot_by_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<RobotRow>, sqlx::Error> {
    let email = name_to_synthetic_email(name);
    let row = sqlx::query!(
        r#"WITH best_token AS (
               SELECT robot_email, revoked, token_prefix, expires_at,
                      first_used_at, last_used_at,
                      created_at AS token_created_at,
                      ROW_NUMBER() OVER (PARTITION BY robot_email
                                         ORDER BY revoked ASC, id DESC) AS rn
               FROM robot_tokens
           )
           SELECT u.email              AS "email!",
                  u.name               AS "display_name!",
                  u.robot_description,
                  u.active             AS "active!",
                  u.created_at         AS "created_at!",
                  bt.revoked           AS "token_revoked?: i64",
                  bt.token_prefix      AS "token_prefix?",
                  bt.expires_at        AS "token_expires_at?",
                  bt.first_used_at     AS "token_first_used_at?",
                  bt.last_used_at      AS "token_last_used_at?",
                  bt.token_created_at  AS "token_created_at?"
           FROM users u
           LEFT JOIN best_token bt
             ON bt.robot_email = u.email AND bt.rn = 1
           WHERE u.email = ? AND u.is_robot = 1"#,
        email,
    )
    .fetch_optional(pool)
    .await?;

    let now = chrono::Utc::now().timestamp();
    Ok(row.map(|r| {
        classify_robot_row(
            r.email,
            r.display_name,
            r.robot_description,
            r.active != 0,
            r.created_at,
            r.token_revoked,
            r.token_prefix,
            r.token_expires_at,
            r.token_first_used_at,
            r.token_last_used_at,
            r.token_created_at,
            now,
        )
    }))
}

/// Classify a row from the `best_token` CTE join into a `RobotRow`.
///
/// Arguments follow the column order of the CTE-joined query so both
/// `list_robots` and `get_robot_by_name` can share a single classifier.
#[allow(clippy::too_many_arguments)]
fn classify_robot_row(
    email: String,
    display_name: String,
    description: Option<String>,
    active: bool,
    created_at: i64,
    token_revoked: Option<i64>,
    token_prefix: Option<String>,
    token_expires_at: Option<i64>,
    token_first_used_at: Option<i64>,
    token_last_used_at: Option<i64>,
    token_created_at: Option<i64>,
    now: i64,
) -> RobotRow {
    let token_state = match token_revoked {
        None => "none",
        Some(1) => "revoked",
        Some(_) => match token_expires_at {
            Some(exp) if exp <= now => "expired",
            _ => "active",
        },
    }
    .to_string();

    RobotRow {
        email,
        display_name,
        description,
        active,
        created_at,
        token_state,
        token_prefix,
        token_expires_at,
        token_first_used_at,
        token_last_used_at,
        token_created_at,
    }
}

/// Create a new robot account on an open connection. The caller must have
/// already acquired a write lock (e.g. via `BEGIN IMMEDIATE`).
async fn create_robot_in_conn(
    conn: &mut SqliteConnection,
    name: &str,
    description: Option<&str>,
    expires_at: Option<i64>,
    token_hash: &str,
    token_prefix: &str,
    roles: &[&str],
) -> Result<(), sqlx::Error> {
    let email = name_to_synthetic_email(name);
    let display_name = format!("robot:{name}");

    sqlx::query!(
        "INSERT INTO users (email, name, is_robot, robot_description) VALUES (?, ?, 1, ?)",
        email,
        display_name,
        description,
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "INSERT INTO robot_tokens (robot_email, token_hash, token_prefix, expires_at)
         VALUES (?, ?, ?, ?)",
        email,
        token_hash,
        token_prefix,
        expires_at,
    )
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

/// Re-activate a tombstoned robot on an open connection. The caller must
/// have already acquired a write lock (e.g. via `BEGIN IMMEDIATE`).
///
/// Per design § REST API step 4, revive treats the row as a "semantically
/// new identity": `active`, `name`, `robot_description`, `created_at`, and
/// `default_channel_id` are all rewritten so no state leaks from the prior
/// identity. `builds.user_email` attribution is preserved because the
/// `users` row itself is kept.
#[allow(clippy::too_many_arguments)]
async fn revive_robot_in_conn(
    conn: &mut SqliteConnection,
    email: &str,
    name: &str,
    description: Option<&str>,
    expires_at: Option<i64>,
    token_hash: &str,
    token_prefix: &str,
    roles: &[&str],
) -> Result<(), sqlx::Error> {
    // Step order matches design § REST API step 3:
    //   DELETE user_roles → DELETE robot_tokens → UPDATE users
    //   → INSERT user_roles → INSERT robot_tokens.
    //
    // Tokens from the prior identity are DELETEd (not revoked) so the
    // revived robot starts with an empty robot_tokens history. The active
    // builds.user_email attribution survives either way because
    // builds.user_email is a free-form string, not an FK into
    // robot_tokens.
    let display_name = format!("robot:{name}");

    sqlx::query!("DELETE FROM user_roles WHERE user_email = ?", email)
        .execute(&mut *conn)
        .await?;

    sqlx::query!("DELETE FROM robot_tokens WHERE robot_email = ?", email)
        .execute(&mut *conn)
        .await?;

    sqlx::query!(
        "UPDATE users
         SET active = 1,
             name = ?,
             robot_description = ?,
             default_channel_id = NULL,
             created_at = unixepoch(),
             updated_at = unixepoch()
         WHERE email = ?",
        display_name,
        description,
        email,
    )
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

    sqlx::query!(
        "INSERT INTO robot_tokens (robot_email, token_hash, token_prefix, expires_at)
         VALUES (?, ?, ?, ?)",
        email,
        token_hash,
        token_prefix,
        expires_at,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Create a fresh robot account or revive a tombstoned one under a
/// `BEGIN IMMEDIATE` transaction. The users row is re-read after the write
/// lock is acquired, so two concurrent requests for the same tombstoned
/// name cannot both pass the "inactive" check — the loser observes the
/// winner's commit and returns [`CreateRobotError::AlreadyActive`]. Any
/// residual UNIQUE constraint violation is normalised to
/// [`CreateRobotError::UniqueViolation`] so callers never surface a raw 500.
pub async fn create_or_revive(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    expires_at: Option<i64>,
    token_hash: &str,
    token_prefix: &str,
    roles: &[&str],
) -> Result<CreateRevivedOutcome, CreateRobotError> {
    let email = name_to_synthetic_email(name);
    let mut conn = pool.acquire().await?;

    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    match create_or_revive_inner(
        &mut conn,
        &email,
        name,
        description,
        expires_at,
        token_hash,
        token_prefix,
        roles,
    )
    .await
    {
        Ok(outcome) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(outcome)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_or_revive_inner(
    conn: &mut SqliteConnection,
    email: &str,
    name: &str,
    description: Option<&str>,
    expires_at: Option<i64>,
    token_hash: &str,
    token_prefix: &str,
    roles: &[&str],
) -> Result<CreateRevivedOutcome, CreateRobotError> {
    let row = sqlx::query!(
        r#"SELECT active AS "active!", is_robot AS "is_robot!"
           FROM users WHERE email = ?"#,
        email,
    )
    .fetch_optional(&mut *conn)
    .await?;

    match row {
        None => {
            create_robot_in_conn(
                &mut *conn,
                name,
                description,
                expires_at,
                token_hash,
                token_prefix,
                roles,
            )
            .await?;
            Ok(CreateRevivedOutcome::Created)
        }
        Some(r) if r.is_robot == 0 => Err(CreateRobotError::HumanCollision),
        Some(r) if r.active != 0 => Err(CreateRobotError::AlreadyActive),
        Some(_) => {
            revive_robot_in_conn(
                &mut *conn,
                email,
                name,
                description,
                expires_at,
                token_hash,
                token_prefix,
                roles,
            )
            .await?;
            Ok(CreateRevivedOutcome::Revived)
        }
    }
}

/// Tombstone a robot under `BEGIN IMMEDIATE`: set `users.active = 0` and
/// revoke every non-revoked robot_tokens row in one serialised transaction.
/// Returns the number of tokens revoked.
pub async fn tombstone_robot(pool: &SqlitePool, email: &str) -> Result<u64, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    match tombstone_robot_inner(&mut conn, email).await {
        Ok(count) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(count)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}

async fn tombstone_robot_inner(
    conn: &mut SqliteConnection,
    email: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query!(
        "UPDATE users SET active = 0, updated_at = unixepoch() WHERE email = ?",
        email,
    )
    .execute(&mut *conn)
    .await?;

    revoke_all_active_tokens_in_conn(&mut *conn, email).await
}

/// Check whether the robot has at least one non-revoked token.
pub async fn has_non_revoked_token(pool: &SqlitePool, email: &str) -> Result<bool, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT COUNT(*) AS "cnt!: i64" FROM robot_tokens
           WHERE robot_email = ? AND revoked = 0"#,
        email,
    )
    .fetch_one(pool)
    .await?;
    Ok(row.cnt > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn test_pool() -> SqlitePool {
        // Unique shared-cache in-memory DB per test so multiple pool
        // connections see the same rows. Shared-cache is incompatible with
        // WAL, so we leave journal mode at its default.
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let url = format!(
            "file:robots_test_{pid}_{id}?mode=memory&cache=shared",
            pid = std::process::id(),
        );

        let options = SqliteConnectOptions::from_str(&url)
            .expect("valid sqlite URL")
            .pragma("foreign_keys", "ON")
            .pragma("busy_timeout", "5000");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            // keep at least one connection alive for the pool's lifetime,
            // otherwise the shared-cache in-memory DB disappears when the
            // last connection drops between operations
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

    async fn seed_tombstoned_robot(pool: &SqlitePool, name: &str) {
        let email = name_to_synthetic_email(name);
        let display = format!("robot:{name}");
        sqlx::query!(
            "INSERT INTO users (email, name, active, is_robot, robot_description)
             VALUES (?, ?, 0, 1, NULL)",
            email,
            display,
        )
        .execute(pool)
        .await
        .expect("seed users");
        sqlx::query!(
            "INSERT INTO robot_tokens
                 (robot_email, token_hash, token_prefix, expires_at, revoked)
             VALUES (?, 'old-hash', 'oldpfx000000', NULL, 1)",
            email,
        )
        .execute(pool)
        .await
        .expect("seed robot_tokens");
    }

    #[test]
    fn name_synthetic_email_roundtrip() {
        // Happy path.
        let e = name_to_synthetic_email("ci-builder");
        assert_eq!(e, "robot+ci-builder@robots");
        assert_eq!(synthetic_email_to_name(&e), Some("ci-builder"));

        // Non-robot emails return None.
        assert_eq!(synthetic_email_to_name("alice@example.com"), None);
        assert_eq!(synthetic_email_to_name("robot+ci@not-robots.com"), None);
        assert_eq!(synthetic_email_to_name("robot_ci@robots"), None);

        // Bare-name punctuation is preserved through the round-trip.
        for n in ["ab", "x.y-z_a", "A1B2", "CI.NIGHTLY"] {
            let e = name_to_synthetic_email(n);
            assert_eq!(synthetic_email_to_name(&e), Some(n));
        }
    }

    #[test]
    fn validate_robot_name_accepts_valid_shapes() {
        for n in [
            "ab",
            "ci",
            "ci-builder",
            "build_system",
            "x.y-z_a",
            "A1B2",
            "CI.NIGHTLY",
            "robot_42",
            &"a".repeat(64),
        ] {
            assert!(
                validate_robot_name(n).is_ok(),
                "expected '{n}' to be accepted"
            );
        }
    }

    #[test]
    fn validate_robot_name_rejects_invalid_shapes() {
        let cases: &[(&str, &str)] = &[
            ("", "empty"),
            ("a", "single character"),
            (".ab", "leading dot"),
            ("ab.", "trailing dot"),
            ("-ab", "leading hyphen"),
            ("ab-", "trailing hyphen"),
            ("a b", "space"),
            ("ab@cd", "@"),
            ("ab/cd", "slash"),
            ("робот", "non-ASCII"),
        ];
        for (n, label) in cases {
            assert!(
                validate_robot_name(n).is_err(),
                "expected '{n}' ({label}) to be rejected"
            );
        }

        let too_long = "a".repeat(65);
        assert!(validate_robot_name(&too_long).is_err());
    }

    #[tokio::test]
    async fn create_then_revive_happy_path() {
        let pool = test_pool().await;

        let outcome = create_or_revive(&pool, "ci", None, None, "hash1", "pfx000000001", &[]).await;
        assert_eq!(outcome.unwrap(), CreateRevivedOutcome::Created);

        // Tombstone it, then revive.
        let email = name_to_synthetic_email("ci");
        tombstone_robot(&pool, &email).await.expect("tombstone");

        let outcome = create_or_revive(&pool, "ci", None, None, "hash2", "pfx000000002", &[]).await;
        assert_eq!(outcome.unwrap(), CreateRevivedOutcome::Revived);

        let row = get_robot_by_name(&pool, "ci").await.unwrap().unwrap();
        assert!(row.active);
        assert_eq!(row.token_state, "active");
    }

    #[tokio::test]
    async fn second_create_on_active_robot_returns_already_active() {
        let pool = test_pool().await;
        create_or_revive(&pool, "ci", None, None, "hash1", "pfx000000001", &[])
            .await
            .unwrap();
        let result = create_or_revive(&pool, "ci", None, None, "hash2", "pfx000000002", &[]).await;
        assert!(matches!(result, Err(CreateRobotError::AlreadyActive)));
    }

    #[tokio::test]
    async fn human_row_under_synthetic_email_returns_human_collision() {
        let pool = test_pool().await;
        let email = name_to_synthetic_email("ci");
        sqlx::query!(
            "INSERT INTO users (email, name, is_robot) VALUES (?, 'someone', 0)",
            email,
        )
        .execute(&pool)
        .await
        .unwrap();

        let result = create_or_revive(&pool, "ci", None, None, "hash1", "pfx000000001", &[]).await;
        assert!(matches!(result, Err(CreateRobotError::HumanCollision)));
    }

    #[tokio::test]
    async fn revive_resets_name_and_default_channel() {
        let pool = test_pool().await;

        // Seed a channel so we can point default_channel_id at something real.
        let channel_id = sqlx::query!("INSERT INTO channels (name, description) VALUES ('c1', '')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();

        let email = name_to_synthetic_email("ci");
        // Prior identity: active=0, old created_at, custom name, channel set.
        sqlx::query!(
            "INSERT INTO users (email, name, active, is_robot, default_channel_id, created_at)
             VALUES (?, 'robot:STALE', 0, 1, ?, 1)",
            email,
            channel_id,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO robot_tokens
                 (robot_email, token_hash, token_prefix, revoked)
             VALUES (?, 'old-hash', 'oldpfx000000', 1)",
            email,
        )
        .execute(&pool)
        .await
        .unwrap();

        let outcome = create_or_revive(&pool, "ci", None, None, "newhash", "newpfx000001", &[])
            .await
            .unwrap();
        assert_eq!(outcome, CreateRevivedOutcome::Revived);

        let row = sqlx::query!(
            r#"SELECT name AS "name!", default_channel_id, created_at AS "created_at!"
               FROM users WHERE email = ?"#,
            email,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.name, "robot:ci", "revive must reset display name");
        assert_eq!(
            row.default_channel_id, None,
            "revive must clear default_channel_id"
        );
        assert!(
            row.created_at > 1,
            "revive must reset created_at to current epoch"
        );
    }

    #[tokio::test]
    async fn concurrent_revive_yields_exactly_one_winner() {
        let pool = test_pool().await;
        seed_tombstoned_robot(&pool, "ci").await;

        let mut handles = Vec::new();
        for i in 0..4 {
            let pool = pool.clone();
            let hash = format!("hash-{i:020x}");
            let prefix = format!("pfx{i:09}"); // 12 chars
            handles.push(tokio::spawn(async move {
                create_or_revive(&pool, "ci", None, None, &hash, &prefix, &[]).await
            }));
        }

        let mut revived = 0;
        let mut conflicts = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(CreateRevivedOutcome::Revived) => revived += 1,
                Ok(CreateRevivedOutcome::Created) => panic!("unexpected: fresh create"),
                Err(CreateRobotError::AlreadyActive) => conflicts += 1,
                Err(CreateRobotError::UniqueViolation) => conflicts += 1,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert_eq!(revived, 1, "exactly one caller must win the revive race");
        assert_eq!(
            conflicts, 3,
            "losers must return 409 (AlreadyActive or UniqueViolation), never 500"
        );

        let row = get_robot_by_name(&pool, "ci").await.unwrap().unwrap();
        assert!(row.active);
        assert_eq!(row.token_state, "active");
    }

    #[tokio::test]
    async fn revive_deletes_prior_robot_tokens_rows() {
        let pool = test_pool().await;

        // Prior identity: tombstoned robot with one revoked token row.
        seed_tombstoned_robot(&pool, "ci").await;
        let email = name_to_synthetic_email("ci");
        let prior_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM robot_tokens WHERE robot_email = ?",
            email,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(prior_count, 1, "seed should leave one prior tombstone row");

        // Revive.
        create_or_revive(&pool, "ci", None, None, "newhash", "newpfx000001", &[])
            .await
            .unwrap();

        // The prior tombstone row is gone; only the new active token remains.
        let rows = sqlx::query!(
            r#"SELECT token_prefix AS "token_prefix!", revoked AS "revoked!: i64"
               FROM robot_tokens WHERE robot_email = ?
               ORDER BY id"#,
            email,
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "revive must DELETE prior rows per design § REST API step 3, \
             leaving only the freshly issued token"
        );
        assert_eq!(rows[0].token_prefix, "newpfx000001");
        assert_eq!(rows[0].revoked, 0);
    }

    #[tokio::test]
    async fn mark_robot_token_used_preserves_first_used_at_across_calls() {
        let pool = test_pool().await;
        create_or_revive(&pool, "ci", None, None, "hash0", "pfx000000000", &[])
            .await
            .unwrap();
        let email = name_to_synthetic_email("ci");
        let token_id = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM robot_tokens WHERE robot_email = ? AND revoked = 0"#,
            email,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        mark_robot_token_used(&pool, token_id).await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM robot_tokens WHERE id = ?",
            token_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let first_at = row.first_used_at.expect("first_used_at set on first call");
        assert!(row.last_used_at.is_some());

        sqlx::query!(
            "UPDATE robot_tokens SET last_used_at = 0 WHERE id = ?",
            token_id,
        )
        .execute(&pool)
        .await
        .unwrap();

        mark_robot_token_used(&pool, token_id).await.unwrap();
        let row = sqlx::query!(
            "SELECT first_used_at, last_used_at FROM robot_tokens WHERE id = ?",
            token_id,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.first_used_at, Some(first_at));
        assert_ne!(row.last_used_at, Some(0));
    }
}
