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

//! Periodic garbage collection for build log files.
//!
//! Runs on a daily interval (first tick delayed 24h to avoid double-GC on
//! quick restarts). Deletes log files and `build_logs` rows for terminal
//! builds older than the configured retention period. Build rows are retained
//! as historical records.

use std::path::PathBuf;

use sqlx::{Row, SqlitePool};

/// Start the log GC background task.
///
/// Returns a `JoinHandle` that should be stored in `AppState` and aborted
/// during shutdown. The first tick is delayed by 24 hours (not immediate).
pub fn start_log_gc(
    pool: SqlitePool,
    log_dir: PathBuf,
    retention_days: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let period = tokio::time::Duration::from_secs(24 * 60 * 60);
        let mut interval = tokio::time::interval(period);

        // Skip the first (immediate) tick — delays GC by one full period.
        interval.tick().await;

        loop {
            interval.tick().await;
            tracing::info!("log GC: starting cleanup cycle");

            match run_gc_cycle(&pool, &log_dir, retention_days).await {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!(deleted = count, "log GC: cleanup cycle complete");
                    } else {
                        tracing::debug!("log GC: no expired logs to clean up");
                    }
                }
                Err(e) => {
                    tracing::error!("log GC: cleanup cycle failed: {e}");
                }
            }
        }
    })
}

/// Run a single GC cycle: find and delete expired log files and rows.
///
/// Returns the number of log entries cleaned up.
async fn run_gc_cycle(
    pool: &SqlitePool,
    log_dir: &PathBuf,
    retention_days: u32,
) -> Result<usize, GcError> {
    // Calculate the cutoff timestamp (seconds since epoch).
    let retention_secs = i64::from(retention_days) * 24 * 60 * 60;
    let cutoff = chrono::Utc::now().timestamp() - retention_secs;

    // Find terminal builds with finished_at older than the cutoff that
    // still have a build_logs row.
    //
    // We use a raw query because the terminal states list is static and
    // sqlx does not support binding arrays for IN clauses directly.
    let rows = sqlx::query(
        "SELECT bl.build_id, bl.log_path
         FROM build_logs bl
         JOIN builds b ON b.id = bl.build_id
         WHERE b.state IN ('success', 'failure', 'revoked')
           AND b.finished_at IS NOT NULL
           AND b.finished_at < ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .map_err(GcError::Db)?;

    if rows.is_empty() {
        return Ok(0);
    }

    let mut cleaned = 0usize;

    for row in &rows {
        let build_id: i64 = row.get("build_id");
        let log_path: String = row.get("log_path");

        // Delete the log file from disk.
        let full_path = log_dir.join(&log_path);
        match tokio::fs::remove_file(&full_path).await {
            Ok(()) => {
                tracing::debug!(
                    build_id = build_id,
                    path = %full_path.display(),
                    "log GC: deleted log file"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File already gone (manual cleanup or previous partial GC).
                tracing::debug!(
                    build_id = build_id,
                    "log GC: log file already missing, removing row"
                );
            }
            Err(e) => {
                tracing::warn!(
                    build_id = build_id,
                    path = %full_path.display(),
                    "log GC: failed to delete log file: {e} — skipping row"
                );
                continue;
            }
        }

        // Delete the build_logs row.
        if let Err(e) = sqlx::query("DELETE FROM build_logs WHERE build_id = ?")
            .bind(build_id)
            .execute(pool)
            .await
        {
            tracing::warn!(
                build_id = build_id,
                "log GC: failed to delete build_logs row: {e}"
            );
            continue;
        }

        cleaned += 1;
    }

    Ok(cleaned)
}

/// Errors that can occur during log GC.
#[derive(Debug)]
enum GcError {
    /// Database query error.
    Db(sqlx::Error),
}

impl std::fmt::Display for GcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for GcError {}
