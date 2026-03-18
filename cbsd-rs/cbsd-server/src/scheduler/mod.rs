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

//! Periodic build scheduler.
//!
//! The scheduler loop loads enabled periodic tasks from the database,
//! computes the next fire time for each (from cron expression or retry_at),
//! sleeps until the earliest one, triggers the build, and handles
//! success/failure/retry outcomes.

pub mod tag_format;
pub mod trigger;

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use croner::Cron;

use crate::app::AppState;
use crate::db;
use trigger::TriggerError;

/// Maximum backoff between retries (10 minutes).
const MAX_BACKOFF_SECS: f64 = 600.0;

/// Base retry delay in seconds.
const BASE_RETRY_SECS: f64 = 30.0;

/// Retry multiplier (exponential backoff factor).
const RETRY_MULTIPLIER: f64 = 1.5;

/// Maximum retry count before disabling the task.
const MAX_RETRY_COUNT: i64 = 10;

/// Run the periodic build scheduler loop.
///
/// This function runs indefinitely. It loads enabled tasks from the database,
/// computes the next fire time, sleeps until then (or until notified of a
/// change), and triggers builds as they come due.
///
/// The `notify` handle is used to wake the scheduler when tasks are
/// created/updated/deleted/enabled/disabled, so it reloads immediately
/// rather than waiting for the current sleep to expire.
pub async fn run_scheduler(state: AppState, notify: Arc<tokio::sync::Notify>) {
    // Log initial task count.
    match db::periodic::list_enabled_tasks(&state.pool).await {
        Ok(tasks) => {
            tracing::info!(
                "periodic scheduler started: {} enabled task(s)",
                tasks.len()
            );
        }
        Err(e) => {
            tracing::error!("periodic scheduler failed to load initial tasks: {e}");
        }
    }

    loop {
        // Step 1: Load all enabled tasks.
        let tasks = match db::periodic::list_enabled_tasks(&state.pool).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("scheduler: failed to load enabled tasks: {e}");
                // Wait a bit before retrying to avoid tight error loop.
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                continue;
            }
        };

        // Step 2: Compute next fire time for each task.
        let now_chrono = Utc::now();
        let mut schedule: Vec<(DateTime<Utc>, String)> = Vec::new();

        for task in &tasks {
            // Check retry_at first.
            if let Some(retry_epoch) = task.retry_at {
                let retry_dt = Utc.timestamp_opt(retry_epoch, 0);
                if let chrono::LocalResult::Single(retry_time) = retry_dt {
                    if retry_time > now_chrono {
                        // Retry is in the future — schedule at retry_at.
                        schedule.push((retry_time, task.id.clone()));
                    } else {
                        // Retry is due now or overdue — fire immediately.
                        schedule.push((now_chrono, task.id.clone()));
                    }
                    continue;
                }
                // Invalid retry_at timestamp — fall through to cron.
            }

            // Parse the cron expression and find the next fire time.
            let cron = match Cron::from_str(&task.cron_expr) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        task_id = %task.id,
                        cron_expr = %task.cron_expr,
                        "scheduler: failed to parse cron expression: {e}"
                    );
                    continue;
                }
            };

            // Find the next occurrence from now.
            let next = cron.find_next_occurrence(&now_chrono, false);

            match next {
                Ok(next_time) => {
                    if next_time <= now_chrono {
                        // Missed — get the next future occurrence.
                        tracing::warn!(
                            task_id = %task.id,
                            "scheduler: cron next time is in the past, getting subsequent"
                        );
                        match cron.find_next_occurrence(&next_time, false) {
                            Ok(future_time) => {
                                schedule.push((future_time, task.id.clone()));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    task_id = %task.id,
                                    "scheduler: no future cron occurrence: {e}"
                                );
                            }
                        }
                    } else {
                        schedule.push((next_time, task.id.clone()));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        task_id = %task.id,
                        "scheduler: no next cron occurrence: {e}"
                    );
                }
            }
        }

        // Step 3: Sort by fire time, then task ID for determinism.
        schedule.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        // Step 4: If empty, wait for notification (no tasks to schedule).
        if schedule.is_empty() {
            tracing::debug!("scheduler: no enabled tasks — waiting for notification");
            notify.notified().await;
            continue;
        }

        let (fire_time, task_id) = &schedule[0];
        let fire_time = *fire_time;
        let task_id = task_id.clone();

        // Convert chrono DateTime to tokio Instant for sleep.
        let duration_until = (fire_time - Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        let sleep_until = tokio::time::Instant::now() + duration_until;

        tracing::debug!(
            task_id = %task_id,
            fire_time = %fire_time,
            duration_secs = duration_until.as_secs(),
            "scheduler: sleeping until next fire time"
        );

        // Step 5: Wait for fire time or notification.
        tokio::select! {
            () = tokio::time::sleep_until(sleep_until) => {
                // Fire time reached — proceed to trigger.
            }
            () = notify.notified() => {
                // Tasks changed — reload.
                tracing::debug!("scheduler: notified of task change — reloading");
                continue;
            }
        }

        // Step 7: Re-fetch the task to check it's still enabled.
        let task = match db::periodic::get_task(&state.pool, &task_id).await {
            Ok(Some(t)) if t.enabled => t,
            Ok(Some(_)) => {
                tracing::debug!(
                    task_id = %task_id,
                    "scheduler: task disabled between schedule and fire — skipping"
                );
                continue;
            }
            Ok(None) => {
                tracing::debug!(
                    task_id = %task_id,
                    "scheduler: task deleted between schedule and fire — skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::error!(
                    task_id = %task_id,
                    "scheduler: failed to re-fetch task: {e}"
                );
                continue;
            }
        };

        // Step 8: Trigger the build.
        match trigger::trigger_periodic_build(&state, &task).await {
            // Step 9: Success.
            Ok(build_id) => {
                tracing::info!(
                    task_id = %task.id,
                    build_id = build_id,
                    "scheduler: periodic build triggered successfully"
                );

                if let Err(e) =
                    db::periodic::update_trigger_success(&state.pool, &task.id, build_id).await
                {
                    tracing::error!(
                        task_id = %task.id,
                        "scheduler: failed to update trigger success: {e}"
                    );
                }
            }
            // Step 10: User deactivated — disable the task.
            Err(TriggerError::UserDeactivated) => {
                tracing::warn!(
                    task_id = %task.id,
                    created_by = %task.created_by,
                    "scheduler: task owner deactivated — disabling task"
                );

                if let Err(e) = db::periodic::disable_with_error(
                    &state.pool,
                    &task.id,
                    "task owner deactivated or not found",
                )
                .await
                {
                    tracing::error!(
                        task_id = %task.id,
                        "scheduler: failed to disable task: {e}"
                    );
                }
            }
            // Step 11: Fatal error — disable the task.
            Err(TriggerError::Fatal(msg)) => {
                tracing::error!(
                    task_id = %task.id,
                    error = %msg,
                    "scheduler: fatal error triggering build — disabling task"
                );

                if let Err(e) =
                    db::periodic::disable_with_error(&state.pool, &task.id, &msg).await
                {
                    tracing::error!(
                        task_id = %task.id,
                        "scheduler: failed to disable task: {e}"
                    );
                }
            }
            // Step 12: Transient error — backoff and retry.
            Err(TriggerError::Transient(msg)) => {
                let new_retry_count = task.retry_count + 1;

                if new_retry_count >= MAX_RETRY_COUNT {
                    tracing::error!(
                        task_id = %task.id,
                        retry_count = new_retry_count,
                        error = %msg,
                        "scheduler: max retries exceeded — disabling task"
                    );

                    if let Err(e) = db::periodic::disable_with_error(
                        &state.pool,
                        &task.id,
                        &format!("max retries ({MAX_RETRY_COUNT}) exceeded: {msg}"),
                    )
                    .await
                    {
                        tracing::error!(
                            task_id = %task.id,
                            "scheduler: failed to disable task: {e}"
                        );
                    }
                } else {
                    // Compute exponential backoff: 30 * 1.5^retry_count, capped at 600s.
                    let backoff_secs = (BASE_RETRY_SECS
                        * RETRY_MULTIPLIER.powi(task.retry_count as i32))
                    .min(MAX_BACKOFF_SECS);
                    let retry_at = Utc::now().timestamp() + backoff_secs as i64;

                    tracing::warn!(
                        task_id = %task.id,
                        retry_count = new_retry_count,
                        backoff_secs = backoff_secs,
                        error = %msg,
                        "scheduler: transient error — scheduling retry"
                    );

                    if let Err(e) = db::periodic::update_retry(
                        &state.pool,
                        &task.id,
                        new_retry_count,
                        retry_at,
                        &msg,
                    )
                    .await
                    {
                        tracing::error!(
                            task_id = %task.id,
                            "scheduler: failed to update retry state: {e}"
                        );
                    }
                }
            }
        }

        // Step 13: Loop back to step 1.
    }
}
