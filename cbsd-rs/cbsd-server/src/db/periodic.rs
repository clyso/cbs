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

//! Database operations for periodic build tasks.

use sqlx::SqlitePool;

/// A periodic task record as stored in the database.
#[derive(Debug, Clone)]
pub struct PeriodicTaskRow {
    pub id: String,
    pub cron_expr: String,
    pub tag_format: String,
    pub descriptor: String,
    #[allow(dead_code)]
    pub descriptor_version: i64,
    pub priority: String,
    pub summary: Option<String>,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub retry_count: i64,
    pub retry_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_triggered_at: Option<i64>,
    pub last_build_id: Option<i64>,
}

/// Insert a new periodic task.
pub async fn insert_task(
    pool: &SqlitePool,
    id: &str,
    cron_expr: &str,
    tag_format: &str,
    descriptor: &str,
    priority: &str,
    summary: Option<&str>,
    created_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO periodic_tasks
           (id, cron_expr, tag_format, descriptor, priority, summary, created_by)
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        id,
        cron_expr,
        tag_format,
        descriptor,
        priority,
        summary,
        created_by,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Get a single periodic task by ID.
pub async fn get_task(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<PeriodicTaskRow>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT
               id                 AS "id!",
               cron_expr          AS "cron_expr!",
               tag_format         AS "tag_format!",
               descriptor         AS "descriptor!",
               descriptor_version AS "descriptor_version!",
               priority           AS "priority!",
               summary,
               enabled            AS "enabled!",
               created_by         AS "created_by!",
               created_at         AS "created_at!",
               updated_at         AS "updated_at!",
               retry_count        AS "retry_count!",
               retry_at,
               last_error,
               last_triggered_at,
               last_build_id
           FROM periodic_tasks WHERE id = ?"#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PeriodicTaskRow {
        id: r.id,
        cron_expr: r.cron_expr,
        tag_format: r.tag_format,
        descriptor: r.descriptor,
        descriptor_version: r.descriptor_version,
        priority: r.priority,
        summary: r.summary,
        enabled: r.enabled != 0,
        created_by: r.created_by,
        created_at: r.created_at,
        updated_at: r.updated_at,
        retry_count: r.retry_count,
        retry_at: r.retry_at,
        last_error: r.last_error,
        last_triggered_at: r.last_triggered_at,
        last_build_id: r.last_build_id,
    }))
}

/// List all periodic tasks, ordered by creation time.
pub async fn list_tasks(pool: &SqlitePool) -> Result<Vec<PeriodicTaskRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT
               id                 AS "id!",
               cron_expr          AS "cron_expr!",
               tag_format         AS "tag_format!",
               descriptor         AS "descriptor!",
               descriptor_version AS "descriptor_version!",
               priority           AS "priority!",
               summary,
               enabled            AS "enabled!",
               created_by         AS "created_by!",
               created_at         AS "created_at!",
               updated_at         AS "updated_at!",
               retry_count        AS "retry_count!",
               retry_at,
               last_error,
               last_triggered_at,
               last_build_id
           FROM periodic_tasks ORDER BY created_at"#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| PeriodicTaskRow {
            id: r.id,
            cron_expr: r.cron_expr,
            tag_format: r.tag_format,
            descriptor: r.descriptor,
            descriptor_version: r.descriptor_version,
            priority: r.priority,
            summary: r.summary,
            enabled: r.enabled != 0,
            created_by: r.created_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
            retry_count: r.retry_count,
            retry_at: r.retry_at,
            last_error: r.last_error,
            last_triggered_at: r.last_triggered_at,
            last_build_id: r.last_build_id,
        })
        .collect())
}

/// List only enabled periodic tasks, ordered by creation time.
pub async fn list_enabled_tasks(pool: &SqlitePool) -> Result<Vec<PeriodicTaskRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT
               id                 AS "id!",
               cron_expr          AS "cron_expr!",
               tag_format         AS "tag_format!",
               descriptor         AS "descriptor!",
               descriptor_version AS "descriptor_version!",
               priority           AS "priority!",
               summary,
               enabled            AS "enabled!",
               created_by         AS "created_by!",
               created_at         AS "created_at!",
               updated_at         AS "updated_at!",
               retry_count        AS "retry_count!",
               retry_at,
               last_error,
               last_triggered_at,
               last_build_id
           FROM periodic_tasks WHERE enabled = 1 ORDER BY created_at"#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| PeriodicTaskRow {
            id: r.id,
            cron_expr: r.cron_expr,
            tag_format: r.tag_format,
            descriptor: r.descriptor,
            descriptor_version: r.descriptor_version,
            priority: r.priority,
            summary: r.summary,
            enabled: r.enabled != 0,
            created_by: r.created_by,
            created_at: r.created_at,
            updated_at: r.updated_at,
            retry_count: r.retry_count,
            retry_at: r.retry_at,
            last_error: r.last_error,
            last_triggered_at: r.last_triggered_at,
            last_build_id: r.last_build_id,
        })
        .collect())
}

/// Delete a periodic task by ID. Returns `true` if a row was deleted.
pub async fn delete_task(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM periodic_tasks WHERE id = ?", id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Enable or disable a periodic task. If `clear_retry` is true, also resets
/// retry_count, retry_at, and last_error. Returns `true` if a row was updated.
#[allow(dead_code)]
pub async fn set_enabled(
    pool: &SqlitePool,
    id: &str,
    enabled: bool,
    clear_retry: bool,
) -> Result<bool, sqlx::Error> {
    let enabled_int: i32 = if enabled { 1 } else { 0 };

    if clear_retry {
        let result = sqlx::query!(
            r#"UPDATE periodic_tasks
               SET enabled = ?, retry_count = 0, retry_at = NULL,
                   last_error = NULL, updated_at = unixepoch()
               WHERE id = ?"#,
            enabled_int,
            id,
        )
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    } else {
        let result = sqlx::query!(
            r#"UPDATE periodic_tasks
               SET enabled = ?, updated_at = unixepoch()
               WHERE id = ?"#,
            enabled_int,
            id,
        )
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

/// Record a successful trigger: reset retry state and set the last build ID.
pub async fn update_trigger_success(
    pool: &SqlitePool,
    id: &str,
    build_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE periodic_tasks
           SET retry_count = 0, retry_at = NULL, last_error = NULL,
               last_triggered_at = unixepoch(), last_build_id = ?,
               updated_at = unixepoch()
           WHERE id = ?"#,
        build_id,
        id,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Record a retry attempt with the next retry timestamp and error message.
pub async fn update_retry(
    pool: &SqlitePool,
    id: &str,
    retry_count: i64,
    retry_at: i64,
    last_error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE periodic_tasks
           SET retry_count = ?, retry_at = ?, last_error = ?,
               updated_at = unixepoch()
           WHERE id = ?"#,
        retry_count,
        retry_at,
        last_error,
        id,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Enable a periodic task and reset all retry state (retry_count, retry_at,
/// last_error). Returns `true` if a row was updated.
pub async fn enable_task(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE periodic_tasks
           SET enabled = 1, retry_count = 0, retry_at = NULL,
               last_error = NULL, updated_at = unixepoch()
           WHERE id = ?"#,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Disable a periodic task. Clears retry_at so the scheduler does not
/// attempt to fire it, but preserves retry_count and last_error for
/// diagnostic visibility. Returns `true` if a row was updated.
pub async fn disable_task(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        r#"UPDATE periodic_tasks
           SET enabled = 0, retry_at = NULL, updated_at = unixepoch()
           WHERE id = ?"#,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Disable a periodic task and record an error message. Clears retry_at
/// since the task is no longer scheduled for retry.
pub async fn disable_with_error(
    pool: &SqlitePool,
    id: &str,
    last_error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE periodic_tasks
           SET enabled = 0, retry_at = NULL, last_error = ?,
               updated_at = unixepoch()
           WHERE id = ?"#,
        last_error,
        id,
    )
    .execute(pool)
    .await?;

    Ok(())
}
