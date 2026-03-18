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

//! Database operations for build records and build log metadata.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

/// A build record as stored in the database.
#[derive(Debug, Clone, Serialize)]
pub struct BuildRecord {
    pub id: i64,
    pub descriptor: String,
    pub descriptor_version: i64,
    pub user_email: String,
    pub priority: String,
    pub state: String,
    pub worker_id: Option<String>,
    pub trace_id: Option<String>,
    pub error: Option<String>,
    pub submitted_at: i64,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// Insert a new build in QUEUED state. Returns the auto-generated build ID.
/// `periodic_task_id` is set for scheduler-triggered builds, `None` for manual.
pub async fn insert_build(
    pool: &SqlitePool,
    descriptor_json: &str,
    user_email: &str,
    priority: &str,
    periodic_task_id: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO builds (descriptor, user_email, priority, state, periodic_task_id)
         VALUES (?, ?, ?, 'queued', ?)
         RETURNING id AS "id!""#,
        descriptor_json,
        user_email,
        priority,
        periodic_task_id,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

/// Get a single build by ID.
pub async fn get_build(pool: &SqlitePool, id: i64) -> Result<Option<BuildRecord>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT
                id            AS "id!",
                descriptor    AS "descriptor!",
                descriptor_version AS "descriptor_version!",
                user_email    AS "user_email!",
                priority      AS "priority!",
                state         AS "state!",
                worker_id,
                trace_id,
                error,
                submitted_at  AS "submitted_at!",
                queued_at     AS "queued_at!",
                started_at,
                finished_at
         FROM builds WHERE id = ?"#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| BuildRecord {
        id: r.id,
        descriptor: r.descriptor,
        descriptor_version: r.descriptor_version,
        user_email: r.user_email,
        priority: r.priority,
        state: r.state,
        worker_id: r.worker_id,
        trace_id: r.trace_id,
        error: r.error,
        submitted_at: r.submitted_at,
        queued_at: r.queued_at,
        started_at: r.started_at,
        finished_at: r.finished_at,
    }))
}

/// List builds with optional filters on user email and state.
pub async fn list_builds(
    pool: &SqlitePool,
    user_filter: Option<&str>,
    state_filter: Option<&str>,
) -> Result<Vec<BuildRecord>, sqlx::Error> {
    // Build the query dynamically based on filters.
    let base = "SELECT id, descriptor, descriptor_version, user_email, priority, state,
                       worker_id, trace_id, error, submitted_at, queued_at, started_at, finished_at
                FROM builds";

    let mut conditions: Vec<String> = Vec::new();
    if user_filter.is_some() {
        conditions.push("user_email = ?".to_string());
    }
    if state_filter.is_some() {
        conditions.push("state = ?".to_string());
    }

    let query_str = if conditions.is_empty() {
        format!("{base} ORDER BY id DESC")
    } else {
        format!("{base} WHERE {} ORDER BY id DESC", conditions.join(" AND "))
    };

    let mut query = sqlx::query(&query_str);

    if let Some(user) = user_filter {
        query = query.bind(user.to_string());
    }
    if let Some(state) = state_filter {
        query = query.bind(state.to_string());
    }

    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(row_to_build_record).collect())
}

/// Update a build's state. Optionally sets the error message.
/// Returns `true` if a row was updated.
pub async fn update_build_state(
    pool: &SqlitePool,
    id: i64,
    new_state: &str,
    error: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE builds SET state = ?, error = COALESCE(?, error)
         WHERE id = ?",
        new_state,
        error,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Insert a build log metadata row (for dispatch in Phase 4).
pub async fn insert_build_log_row(
    pool: &SqlitePool,
    build_id: i64,
    log_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO build_logs (build_id, log_path) VALUES (?, ?)",
        build_id,
        log_path,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Set the trace_id and worker_id on a build, and mark it as dispatched.
/// Returns `true` if a row was updated.
pub async fn set_build_dispatched(
    pool: &SqlitePool,
    id: i64,
    trace_id: &str,
    worker_id: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE builds SET state = 'dispatched', trace_id = ?, worker_id = ?
         WHERE id = ?",
        trace_id,
        worker_id,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Mark a build as started and set started_at to the current time.
/// Returns `true` if a row was updated.
pub async fn set_build_started(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE builds SET state = 'started', started_at = unixepoch()
         WHERE id = ?",
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Mark a build as finished (success, failure, or revoked) and set finished_at.
/// Optionally records an error message.
/// Returns `true` if a row was updated.
pub async fn set_build_finished(
    pool: &SqlitePool,
    id: i64,
    state: &str,
    error: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE builds SET state = ?, finished_at = unixepoch(), error = COALESCE(?, error)
         WHERE id = ?",
        state,
        error,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set a build's state to "revoking".
/// Returns `true` if a row was updated.
pub async fn set_build_revoking(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE builds SET state = 'revoking' WHERE id = ?",
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Mark a build log as finished (`build_logs.finished = 1`).
pub async fn set_build_log_finished(pool: &SqlitePool, build_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE build_logs SET finished = 1, updated_at = unixepoch() WHERE build_id = ?",
        build_id,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Map a sqlx Row to a BuildRecord.
///
/// Used by `list_builds` which constructs dynamic SQL and returns untyped rows.
fn row_to_build_record(r: sqlx::sqlite::SqliteRow) -> BuildRecord {
    BuildRecord {
        id: r.get("id"),
        descriptor: r.get("descriptor"),
        descriptor_version: r.get("descriptor_version"),
        user_email: r.get("user_email"),
        priority: r.get("priority"),
        state: r.get("state"),
        worker_id: r.get("worker_id"),
        trace_id: r.get("trace_id"),
        error: r.get("error"),
        submitted_at: r.get("submitted_at"),
        queued_at: r.get("queued_at"),
        started_at: r.get("started_at"),
        finished_at: r.get("finished_at"),
    }
}
