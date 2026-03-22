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

//! Startup recovery: reconcile in-flight builds after a server restart.

use cbsd_proto::{BuildDescriptor, BuildId, Priority};
use sqlx::SqlitePool;

use crate::app::LogWatchers;
use crate::queue::{QueuedBuild, SharedBuildQueue};

/// Recover build state after a server restart.
///
/// Must be called after migrations complete and before accepting connections.
///
/// 1. Builds in `dispatched` or `started` -> mark `failure` (server restarted).
/// 2. Builds in `revoking` -> mark `revoked`, finalize log rows.
/// 3. Builds in `queued` -> re-enqueue into the in-memory priority queue.
/// 4. Clear any stale log watcher entries.
pub async fn run_startup_recovery(
    pool: &SqlitePool,
    queue: &SharedBuildQueue,
    log_watchers: &LogWatchers,
) -> Result<(), sqlx::Error> {
    // 1. Fail in-flight builds (dispatched or started).
    let in_flight_rows =
        sqlx::query!(r#"SELECT id AS "id!" FROM builds WHERE state IN ('dispatched', 'started')"#,)
            .fetch_all(pool)
            .await?;

    let failed_count = in_flight_rows.len();
    for row in &in_flight_rows {
        let id = row.id;
        sqlx::query!(
            "UPDATE builds SET state = 'failure', error = 'server restarted', \
             finished_at = unixepoch() WHERE id = ?",
            id,
        )
        .execute(pool)
        .await?;

        // Finalize the build log so SSE streams close cleanly.
        sqlx::query!(
            "UPDATE build_logs SET finished = 1, updated_at = unixepoch() WHERE build_id = ?",
            id,
        )
        .execute(pool)
        .await?;
    }

    // 2. Finalize revoking builds.
    let revoking_rows = sqlx::query!(r#"SELECT id AS "id!" FROM builds WHERE state = 'revoking'"#,)
        .fetch_all(pool)
        .await?;

    let revoked_count = revoking_rows.len();
    for row in &revoking_rows {
        let id = row.id;
        sqlx::query!(
            "UPDATE builds SET state = 'revoked', finished_at = unixepoch() WHERE id = ?",
            id,
        )
        .execute(pool)
        .await?;

        sqlx::query!(
            "UPDATE build_logs SET finished = 1, updated_at = unixepoch() WHERE build_id = ?",
            id,
        )
        .execute(pool)
        .await?;
    }

    // 3. Re-enqueue queued builds into the in-memory priority queue.
    let queued_rows = sqlx::query!(
        r#"SELECT id AS "id!", descriptor AS "descriptor!",
                  priority AS "priority!", user_email AS "user_email!",
                  queued_at AS "queued_at!"
         FROM builds WHERE state = 'queued' ORDER BY queued_at ASC"#,
    )
    .fetch_all(pool)
    .await?;

    let queued_count = queued_rows.len();
    {
        let mut q = queue.lock().await;
        for row in &queued_rows {
            let id = row.id;
            let descriptor_json = &row.descriptor;
            let priority_str = &row.priority;
            let user_email = row.user_email.clone();
            let queued_at = row.queued_at;

            let priority = match priority_str.as_str() {
                "high" => Priority::High,
                "low" => Priority::Low,
                _ => Priority::Normal,
            };

            let descriptor: BuildDescriptor = match serde_json::from_str(descriptor_json) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!(
                        build_id = id,
                        "failed to deserialize build descriptor during recovery: {e} — \
                         marking as failure"
                    );
                    sqlx::query!(
                        "UPDATE builds SET state = 'failure', \
                         error = 'corrupt descriptor (recovery)', \
                         finished_at = unixepoch() WHERE id = ?",
                        id,
                    )
                    .execute(pool)
                    .await?;
                    continue;
                }
            };

            q.enqueue(QueuedBuild {
                build_id: BuildId(id),
                priority,
                descriptor,
                user_email,
                queued_at,
            });
        }
    }

    // 4. Clear stale log watchers (should be empty on fresh start, but be
    // defensive).
    {
        let mut watchers = log_watchers.lock().await;
        watchers.clear();
    }

    tracing::info!(
        queued = queued_count,
        failed = failed_count,
        revoked = revoked_count,
        "startup recovery complete: recovered {queued_count} queued builds, \
         failed {failed_count} in-flight builds, revoked {revoked_count} revoking builds"
    );

    Ok(())
}
