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

//! Build dispatch engine.
//!
//! `try_dispatch` is the core function: called when a build is submitted or a
//! worker becomes idle. It pops the highest-priority pending build, finds a
//! matching idle worker, updates the DB, packs the component tarball, and
//! sends `BuildNew` + binary tarball to the worker.

use axum::extract::ws::Message;
use cbsd_proto::ws::ServerMessage;
use cbsd_proto::{BuildId, Priority};

use crate::app::AppState;
use crate::components::tarball;
use crate::db;
use crate::queue::{ActiveBuild, QueuedBuild};

/// Errors that can occur during dispatch.
#[derive(Debug)]
pub enum DispatchError {
    /// No pending builds or no idle workers — not an error, just nothing to do.
    NothingToDispatch,
    /// Database error during state transition.
    Database(sqlx::Error),
    /// Failed to pack the component tarball.
    Tarball(std::io::Error),
    /// Failed to send message to worker (channel closed).
    Send(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToDispatch => write!(f, "nothing to dispatch"),
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Tarball(e) => write!(f, "tarball packing error: {e}"),
            Self::Send(e) => write!(f, "send error: {e}"),
        }
    }
}

/// Attempt to dispatch the next pending build to an idle worker.
///
/// This is the core dispatch loop body. It is called:
/// - When a new build is submitted (from the REST handler)
/// - When a worker becomes idle (build finished, build rejected)
///
/// Returns `Ok(())` if a build was dispatched or there was nothing to do.
/// Returns `Err` on database/IO/send failures.
pub async fn try_dispatch(state: &AppState) -> Result<(), DispatchError> {
    // Step 1-4: Under the queue lock, pop a build and find a matching worker.
    let dispatch_info = {
        let mut queue = state.queue.lock().await;

        // Pop highest-priority pending build.
        let build = match queue.next_pending() {
            Some(b) => b,
            None => return Err(DispatchError::NothingToDispatch),
        };

        let build_arch = build.descriptor.build.arch;

        // Find first idle worker with matching arch.
        // A worker is idle if it's Connected and has no active build.
        let worker = queue
            .workers
            .iter()
            .find(|(cid, ws)| {
                ws.is_dispatch_eligible()
                    && ws.arch() == Some(build_arch)
                    && !queue.active.values().any(|ab| ab.connection_id == **cid)
            })
            .map(|(cid, ws)| (cid.clone(), ws.worker_id().unwrap_or("unknown").to_string()));

        let (connection_id, worker_id) = match worker {
            Some(w) => w,
            None => {
                // No matching worker — push build back to front of its lane.
                queue.enqueue_front(build);
                return Err(DispatchError::NothingToDispatch);
            }
        };

        // Step 5: Generate trace_id.
        let trace_id = uuid::Uuid::new_v4().to_string();

        // Step 6: Update DB under the lock (correctness invariant #1).
        db::builds::set_build_dispatched(
            &state.pool,
            build.build_id.0,
            &trace_id,
            &worker_id,
        )
        .await
        .map_err(DispatchError::Database)?;

        // Insert build log row.
        let log_path = format!(
            "builds/{}/build.log",
            build.build_id.0
        );
        db::builds::insert_build_log_row(&state.pool, build.build_id.0, &log_path)
            .await
            .map_err(DispatchError::Database)?;

        // Step 7: Create watch channel for log notifications.
        let (watch_tx, _watch_rx) = tokio::sync::watch::channel(());
        {
            let mut watchers = state.log_watchers.lock().await;
            watchers.insert(build.build_id.0, watch_tx);
        }

        // Step 8: Insert ActiveBuild into queue.active.
        let ack_cancel = tokio_util::sync::CancellationToken::new();
        queue.active.insert(
            build.build_id.0,
            ActiveBuild {
                build_id: build.build_id.0,
                connection_id: connection_id.clone(),
                dispatched_at: tokio::time::Instant::now(),
                trace_id: trace_id.clone(),
                descriptor: build.descriptor.clone(),
                priority: build.priority,
                ack_cancel: ack_cancel.clone(),
            },
        );

        tracing::info!(
            build_id = build.build_id.0,
            connection_id = %connection_id,
            worker_id = %worker_id,
            trace_id = %trace_id,
            arch = %build_arch,
            "build dispatched to worker"
        );

        // Collect info needed outside the lock.
        DispatchInfo {
            build_id: build.build_id,
            priority: build.priority,
            descriptor: build.descriptor.clone(),
            trace_id,
            connection_id,
        }
    };
    // Step 9: Lock released here.

    // Step 10: Pack component tarball (outside lock).
    let component_name = dispatch_info
        .descriptor
        .components
        .first()
        .map(|c| c.name.as_str())
        .unwrap_or("unknown");

    let component_dir = state.config.components_dir.join(component_name);
    let (tar_gz_bytes, sha256_hex) = tarball::pack_component(&component_dir, component_name)
        .map_err(DispatchError::Tarball)?;

    tracing::debug!(
        build_id = dispatch_info.build_id.0,
        component = component_name,
        tarball_size = tar_gz_bytes.len(),
        sha256 = %sha256_hex,
        "component tarball packed"
    );

    // Step 11: Send BuildNew JSON frame + binary tarball frame to worker.
    let build_new = ServerMessage::BuildNew {
        build_id: dispatch_info.build_id,
        trace_id: dispatch_info.trace_id.clone(),
        priority: dispatch_info.priority,
        descriptor: dispatch_info.descriptor.clone(),
        component_sha256: sha256_hex,
    };

    let json_text =
        serde_json::to_string(&build_new).expect("ServerMessage serialization cannot fail");

    let send_result = {
        let senders = state.worker_senders.lock().await;
        if let Some(tx) = senders.get(&dispatch_info.connection_id) {
            tx.send(Message::Text(json_text.into()))
                .and_then(|()| tx.send(Message::Binary(tar_gz_bytes.into())))
                .map_err(|e| DispatchError::Send(e.to_string()))
        } else {
            Err(DispatchError::Send(format!(
                "no sender for connection {}",
                dispatch_info.connection_id
            )))
        }
    };

    // Step 12: On send failure, roll back.
    if let Err(e) = send_result {
        tracing::error!(
            build_id = dispatch_info.build_id.0,
            connection_id = %dispatch_info.connection_id,
            "failed to send build to worker: {e}"
        );

        let mut queue = state.queue.lock().await;
        queue.active.remove(&dispatch_info.build_id.0);

        // Re-queue the build at front of its priority lane.
        queue.enqueue_front(QueuedBuild {
            build_id: dispatch_info.build_id,
            priority: dispatch_info.priority,
            descriptor: dispatch_info.descriptor,
            user_email: String::new(), // not used for re-queue ordering
            queued_at: 0,              // not used for re-queue ordering
        });

        // Remove watch sender.
        let mut watchers = state.log_watchers.lock().await;
        watchers.remove(&dispatch_info.build_id.0);

        return Err(e);
    }

    // Step 13: Spawn ack timeout task. If the worker doesn't send
    // build_accepted within dispatch_ack_timeout_secs, re-queue the build.
    // The CancellationToken in ActiveBuild is cancelled by handle_build_accepted.
    {
        let ack_timeout_secs = state.config.timeouts.dispatch_ack_timeout_secs;
        let ack_state = state.clone();
        let ack_build_id = dispatch_info.build_id;
        let cancel = {
            let queue = state.queue.lock().await;
            queue
                .active
                .get(&ack_build_id.0)
                .map(|a| a.ack_cancel.clone())
        };
        if let Some(cancel) = cancel {
            tokio::spawn(async move {
                tokio::select! {
                    () = cancel.cancelled() => {
                        // build_accepted received, nothing to do
                    }
                    () = tokio::time::sleep(std::time::Duration::from_secs(ack_timeout_secs)) => {
                        tracing::warn!(
                            build_id = ack_build_id.0,
                            timeout_secs = ack_timeout_secs,
                            "dispatch ack timeout — re-queuing build"
                        );
                        let mut queue = ack_state.queue.lock().await;
                        if let Some(active) = queue.active.remove(&ack_build_id.0) {
                            queue.enqueue_front(QueuedBuild {
                                build_id: ack_build_id,
                                priority: active.priority,
                                descriptor: active.descriptor,
                                user_email: String::new(),
                                queued_at: 0,
                            });
                        }
                        drop(queue);
                        let _ = db::builds::update_build_state(
                            &ack_state.pool, ack_build_id.0, "queued", None,
                        ).await;
                        // Re-dispatch will be picked up by the periodic sweep
                        // (30s) or the next build_finished event.
                    }
                }
            });
        }
    }

    Ok(())
}

/// Handle a `BuildAccepted` message from a worker.
///
/// Cancels the dispatch ack timeout.
pub async fn handle_build_accepted(
    state: &AppState,
    connection_id: &str,
    build_id: i64,
) {
    let queue = state.queue.lock().await;
    if let Some(active) = queue.active.get(&build_id) {
        active.ack_cancel.cancel();
        tracing::info!(
            build_id = build_id,
            connection_id = %connection_id,
            "build accepted by worker, ack timer cancelled"
        );
    } else {
        tracing::warn!(
            build_id = build_id,
            connection_id = %connection_id,
            "build_accepted for unknown active build"
        );
    }
}

/// Handle a `BuildStarted` message from a worker.
///
/// Updates the DB state to "started" and sets `started_at`.
pub async fn handle_build_started(state: &AppState, build_id: i64) {
    match db::builds::set_build_started(&state.pool, build_id).await {
        Ok(true) => {
            tracing::info!(build_id = build_id, "build started");
        }
        Ok(false) => {
            tracing::warn!(
                build_id = build_id,
                "build started but no DB row updated (stale?)"
            );
        }
        Err(e) => {
            tracing::error!(
                build_id = build_id,
                "failed to update build state to started: {e}"
            );
        }
    }
}

/// Handle a `BuildFinished` message from a worker.
///
/// Updates DB state (success/failure/revoked), removes from `queue.active`,
/// drops the watch sender from `log_watchers`, and attempts to dispatch the
/// next queued build if the worker is now idle.
pub async fn handle_build_finished(
    state: &AppState,
    connection_id: &str,
    build_id: i64,
    status: &str,
    error: Option<&str>,
) {
    // Update DB.
    match db::builds::set_build_finished(&state.pool, build_id, status, error).await {
        Ok(true) => {
            tracing::info!(
                build_id = build_id,
                status = status,
                "build finished"
            );
        }
        Ok(false) => {
            tracing::warn!(
                build_id = build_id,
                status = status,
                "build finished but no DB row updated (stale?)"
            );
        }
        Err(e) => {
            tracing::error!(
                build_id = build_id,
                "failed to update build state to {status}: {e}"
            );
        }
    }

    // Remove from active builds.
    {
        let mut queue = state.queue.lock().await;
        queue.active.remove(&build_id);
    }

    // Drop watch sender (signals SSE followers that the log is done).
    {
        let mut watchers = state.log_watchers.lock().await;
        watchers.remove(&build_id);
    }

    // Worker is now idle — try to dispatch the next queued build.
    tracing::debug!(
        connection_id = %connection_id,
        "worker idle after build {build_id} — attempting next dispatch"
    );
    if let Err(DispatchError::NothingToDispatch) = try_dispatch(state).await {
        tracing::debug!("no pending builds to dispatch");
    }
}

/// Handle a `BuildRejected` message from a worker.
///
/// If the reason contains "integrity", the build is marked as FAILURE (bad
/// tarball, not worth retrying). Otherwise, the build is re-queued at the
/// front of its priority lane and dispatch is retried with the next worker.
pub async fn handle_build_rejected(
    state: &AppState,
    connection_id: &str,
    build_id: i64,
    reason: &str,
) {
    if reason.to_lowercase().contains("integrity") {
        // Integrity failure — mark as failed, do not re-queue.
        tracing::error!(
            build_id = build_id,
            connection_id = %connection_id,
            reason = %reason,
            "build rejected (integrity failure) — marking as failed"
        );

        if let Err(e) =
            db::builds::set_build_finished(&state.pool, build_id, "failure", Some(reason)).await
        {
            tracing::error!(
                build_id = build_id,
                "failed to update build state to failure: {e}"
            );
        }

        // Remove from active builds and log watchers.
        {
            let mut queue = state.queue.lock().await;
            queue.active.remove(&build_id);
        }
        {
            let mut watchers = state.log_watchers.lock().await;
            watchers.remove(&build_id);
        }
    } else {
        // Transient rejection — re-queue at front.
        tracing::warn!(
            build_id = build_id,
            connection_id = %connection_id,
            reason = %reason,
            "build rejected — re-queuing"
        );

        // Extract the build info from active, then re-queue.
        let active_build = {
            let mut queue = state.queue.lock().await;
            queue.active.remove(&build_id)
        };

        if let Some(ab) = active_build {
            ab.ack_cancel.cancel(); // cancel ack timer if still running
            let mut queue = state.queue.lock().await;
            queue.enqueue_front(QueuedBuild {
                build_id: BuildId(build_id),
                priority: ab.priority,
                descriptor: ab.descriptor,
                user_email: String::new(),
                queued_at: 0,
            });
        }

        // Revert DB state to queued.
        if let Err(e) =
            db::builds::update_build_state(&state.pool, build_id, "queued", None).await
        {
            tracing::error!(
                build_id = build_id,
                "failed to revert build state to queued: {e}"
            );
        }

        // Remove watch sender (will be re-created on next dispatch).
        {
            let mut watchers = state.log_watchers.lock().await;
            watchers.remove(&build_id);
        }

        // Try to dispatch to another worker.
        if let Err(DispatchError::NothingToDispatch) = try_dispatch(state).await {
            tracing::debug!("no workers available to retry rejected build {build_id}");
        }
    }
}

/// Collected info from the queue lock needed for tarball packing and sending.
struct DispatchInfo {
    build_id: BuildId,
    priority: Priority,
    descriptor: cbsd_proto::BuildDescriptor,
    trace_id: String,
    connection_id: String,
}
