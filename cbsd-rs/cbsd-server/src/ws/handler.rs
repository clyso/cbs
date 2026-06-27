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

//! Per-connection WebSocket handler for worker connections.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

use cbsd_proto::BuildId;
use cbsd_proto::ws::{BuildFinishedStatus, ServerMessage, WorkerMessage, WorkerReportedState};

use sqlx::SqlitePool;

use crate::app::{AppState, LogWatchers};
use crate::db;
use crate::db::workers::WorkerRow;
use crate::queue::{ActiveAssignmentReceipt, SharedBuildQueue};
use crate::ws::dispatch;
use crate::ws::liveness::WorkerState;

/// HTTP upgrade handler for `GET /ws/worker`.
///
/// Auth is performed manually from the upgrade request headers because the
/// `AuthUser` extractor targets REST endpoints, not WebSocket upgrades.
///
/// After API key verification, looks up the registered worker bound to that
/// key. Unregistered API keys are rejected with 403.
pub async fn ws_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, StatusCode> {
    // Extract and verify API key from Authorization header
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Only API keys (cbsk_ prefix) are accepted for worker connections
    if !token.starts_with("cbsk_") {
        tracing::warn!("ws upgrade rejected: non-API-key token");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let cached = crate::auth::token_cache::verify_api_key(&state.pool, &state.token_cache, token)
        .await
        .map_err(|e| {
            tracing::warn!("ws upgrade rejected: {e}");
            StatusCode::UNAUTHORIZED
        })?;

    // Look up the registered worker bound to this API key
    let worker_row = db::workers::get_worker_by_api_key_id(&state.pool, cached.token_id)
        .await
        .map_err(|e| {
            tracing::error!("ws upgrade: DB error looking up worker: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::warn!(
                token_id = cached.token_id,
                "ws upgrade rejected: API key is not bound to a registered worker"
            );
            StatusCode::FORBIDDEN
        })?;

    // Generate a server-assigned connection UUID
    let connection_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(
        connection_id = %connection_id,
        worker_id = %worker_row.id,
        worker_name = %worker_row.name,
        "ws upgrade accepted"
    );

    // Per audit-rem D6: bound WS message and frame sizes on the
    // server-accept side. tungstenite defaults (64 MiB / 16 MiB) are
    // larger than anything the protocol legitimately needs.
    let ws = ws
        .max_message_size(cbsd_common::limits::WS_MAX_MESSAGE_BYTES)
        .max_frame_size(cbsd_common::limits::WS_MAX_FRAME_BYTES);

    Ok(ws.on_upgrade(move |socket| handle_connection(socket, state, connection_id, worker_row)))
}

/// Main per-connection loop. Runs until the WebSocket closes.
async fn handle_connection(
    socket: WebSocket,
    state: AppState,
    connection_id: String,
    worker_row: WorkerRow,
) {
    let (mut sender, mut receiver) = socket.split();

    // Step 1: Wait for the hello message (first text frame).
    let hello = match wait_for_hello(&mut receiver).await {
        Ok(h) => h,
        Err(reason) => {
            tracing::warn!(
                connection_id = %connection_id,
                worker_name = %worker_row.name,
                "ws handshake failed: {reason}"
            );
            let _ = send_json(
                &mut sender,
                &ServerMessage::Error {
                    reason,
                    min_version: Some(2),
                    max_version: Some(2),
                },
            )
            .await;
            return;
        }
    };

    // Step 2: Validate protocol version and arch.
    let (arch, cores_total, ram_total_mb, worker_version) = match hello {
        WorkerMessage::Hello {
            protocol_version,
            arch,
            cores_total,
            ram_total_mb,
            version,
        } => {
            if protocol_version != 2 {
                let reason =
                    format!("unsupported protocol version {protocol_version}; server supports 2");
                tracing::warn!(
                    connection_id = %connection_id,
                    worker_name = %worker_row.name,
                    "{reason}"
                );
                let _ = send_json(
                    &mut sender,
                    &ServerMessage::Error {
                        reason,
                        min_version: Some(2),
                        max_version: Some(2),
                    },
                )
                .await;
                return;
            }

            // Validate arch against registered value. If the DB contains an
            // unrecognizable arch string (corruption), reject rather than
            // silently falling back.
            let Ok(registered_arch) = serde_json::from_value::<cbsd_proto::Arch>(
                serde_json::Value::String(worker_row.arch.clone()),
            ) else {
                let reason = format!(
                    "worker '{}' has invalid arch '{}' in database — cannot validate",
                    worker_row.name, worker_row.arch
                );
                tracing::error!(
                    connection_id = %connection_id,
                    worker_name = %worker_row.name,
                    db_arch = %worker_row.arch,
                    "{reason}"
                );
                let _ = send_json(
                    &mut sender,
                    &ServerMessage::Error {
                        reason,
                        min_version: None,
                        max_version: None,
                    },
                )
                .await;
                return;
            };

            if arch != registered_arch {
                let reason = format!(
                    "arch mismatch: worker '{}' registered as {} but reported {} \
                     — re-register with correct arch or fix the worker token",
                    worker_row.name, worker_row.arch, arch
                );
                tracing::error!(
                    connection_id = %connection_id,
                    worker_name = %worker_row.name,
                    registered_arch = %worker_row.arch,
                    reported_arch = %arch,
                    "arch mismatch — disconnecting"
                );
                let _ = send_json(
                    &mut sender,
                    &ServerMessage::Error {
                        reason,
                        min_version: None,
                        max_version: None,
                    },
                )
                .await;
                return;
            }

            (arch, cores_total, ram_total_mb, version)
        }
        other => {
            let reason = format!("expected hello, got {:?}", message_type_name(&other));
            tracing::warn!(
                connection_id = %connection_id,
                "{reason}"
            );
            let _ = send_json(
                &mut sender,
                &ServerMessage::Error {
                    reason,
                    min_version: None,
                    max_version: None,
                },
            )
            .await;
            return;
        }
    };

    let worker_name = worker_row.name.clone();
    let registered_worker_id = worker_row.id.clone();

    tracing::info!(
        connection_id = %connection_id,
        worker_id = %registered_worker_id,
        worker_name = %worker_name,
        arch = %arch,
        cores = cores_total,
        ram_mb = ram_total_mb,
        worker_version = worker_version.as_deref().unwrap_or("unknown"),
        "worker connected"
    );

    // Check version skew between worker and server.
    match worker_version {
        Some(ref wv) if wv != crate::VERSION => {
            tracing::warn!(
                worker_name = %worker_name,
                worker_version = %wv,
                server_version = crate::VERSION,
                "worker/server version mismatch"
            );
        }
        None => {
            tracing::debug!(
                worker_name = %worker_name,
                "worker did not report version"
            );
        }
        _ => {}
    }

    // Step 2b: Update last_seen.
    if let Err(e) = db::workers::update_last_seen(&state.pool, &registered_worker_id).await {
        tracing::warn!(worker_id = %registered_worker_id, "failed to update last_seen: {e}");
    }

    // Step 3: Connection migration — check for existing entry with same
    // registered_worker_id (reconnection or stale double-connect).
    let old_connection_to_cleanup: Option<(String, bool, Vec<i64>)> = {
        let mut queue = state.queue.lock().await;
        let old = queue.workers.iter().find_map(|(cid, ws)| {
            if ws.registered_worker_id() == Some(&registered_worker_id) && *cid != connection_id {
                Some((cid.clone(), matches!(ws, WorkerState::Connected { .. })))
            } else {
                None
            }
        });

        let result = if let Some((old_cid, was_connected)) = old {
            // Migrate active build references from old to new connection,
            // capturing which builds were owned by the old connection so D13
            // can send a migration-supersede revoke on the old sender below.
            let mut migrated_builds = Vec::new();
            for ab in queue.active.values_mut() {
                if ab.connection_id == old_cid {
                    migrated_builds.push(ab.build_id);
                    ab.connection_id = connection_id.clone();
                }
            }
            // Remove old entry
            queue.workers.remove(old_cid.as_str());

            tracing::info!(
                old_connection = %old_cid,
                new_connection = %connection_id,
                was_connected = was_connected,
                migrated_builds = migrated_builds.len(),
                "migrated worker '{}' to new connection",
                worker_name,
            );
            crate::metrics::lifecycle::record_worker_reconnect(&registered_worker_id);
            Some((old_cid, was_connected, migrated_builds))
        } else {
            None
        };

        // Register new connection
        queue.register_worker(
            connection_id.clone(),
            WorkerState::Connected {
                registered_worker_id: registered_worker_id.clone(),
                worker_name: worker_name.clone(),
                arch,
                cores_total,
                ram_total_mb,
                version: worker_version,
            },
        );

        result
    };
    // Queue lock released here.

    // Clean up old connection's sender (after releasing queue lock to avoid
    // lock inversion — cleanup_worker acquires worker_senders first, then queue).
    if let Some((old_cid, was_connected, migrated_builds)) = old_connection_to_cleanup {
        // audit-rem D13 (option A): best-effort stop-work to the superseded
        // connection, then remove its sender — a single helper under one lock so
        // the send-before-remove ordering can't be lost to a refactor. A no-op
        // revoke in the common reconnect case where the old socket is already
        // gone (single-connection worker); the sender is removed either way. See
        // design 019 v2.
        dispatch::revoke_and_remove_superseded(&state.worker_senders, &old_cid, &migrated_builds)
            .await;

        if was_connected {
            // Stale double-connect: re-queue any orphaned build from old connection.
            handle_worker_dead(&state, &old_cid).await;
        }
    }

    // Step 3b: Register an outbound message channel for this worker.
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    {
        let mut senders = state.worker_senders.lock().await;
        senders.insert(connection_id.clone(), outbound_tx);
    }

    // Step 4: Send welcome message.
    let grace_period_secs = state.config.timeouts.liveness_grace_period_secs;
    if let Err(e) = send_json(
        &mut sender,
        &ServerMessage::Welcome {
            protocol_version: 2,
            connection_id: connection_id.clone(),
            grace_period_secs,
            // Metrics push is not yet advertised; G5 sets this from the
            // server's metrics config so workers only push when wanted.
            accepts_metrics: false,
        },
    )
    .await
    {
        tracing::error!(
            connection_id = %connection_id,
            "failed to send welcome: {e}"
        );
        cleanup_worker(
            &state,
            &connection_id,
            &worker_name,
            &registered_worker_id,
            false,
        )
        .await;
        return;
    }

    // Step 4b: Try to dispatch a queued build to this newly connected worker.
    {
        let state_clone = state.clone();
        let cid = connection_id.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatch::try_dispatch(&state_clone).await {
                tracing::debug!(
                    connection_id = %cid,
                    "post-connect dispatch: {e}"
                );
            }
        });
    }

    // Step 5: Message loop.
    use futures_util::{SinkExt, StreamExt};

    let forward_task = async {
        while let Some(msg) = outbound_rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    };

    let receive_task = async {
        while let Some(result) = receiver.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        connection_id = %connection_id,
                        worker_name = %worker_name,
                        "ws receive error: {e}"
                    );
                    break;
                }
            };

            match msg {
                Message::Text(text) => {
                    let parsed: Result<WorkerMessage, _> = serde_json::from_str(&text);
                    match parsed {
                        Ok(worker_msg) => {
                            handle_worker_message(
                                &state,
                                &connection_id,
                                &worker_name,
                                &registered_worker_id,
                                worker_msg,
                            )
                            .await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                connection_id = %connection_id,
                                worker_name = %worker_name,
                                "failed to parse worker message: {e}"
                            );
                        }
                    }
                }
                Message::Close(_) => {
                    tracing::info!(
                        connection_id = %connection_id,
                        worker_name = %worker_name,
                        "ws close frame received"
                    );
                    break;
                }
                _ => {}
            }
        }
    };

    tokio::select! {
        () = forward_task => {
            tracing::debug!(
                connection_id = %connection_id,
                "outbound channel closed"
            );
        }
        () = receive_task => {
            tracing::debug!(
                connection_id = %connection_id,
                "inbound stream ended"
            );
        }
    }

    // Step 6: Connection closed — determine final state.
    let is_stopping = {
        let queue = state.queue.lock().await;
        matches!(
            queue.get_worker(&connection_id),
            Some(WorkerState::Stopping { .. })
        )
    };
    cleanup_worker(
        &state,
        &connection_id,
        &worker_name,
        &registered_worker_id,
        is_stopping,
    )
    .await;
}

/// Wait for the first text frame and parse it as a `WorkerMessage`.
async fn wait_for_hello(
    receiver: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Result<WorkerMessage, String> {
    use futures_util::StreamExt;

    let timeout = tokio::time::Duration::from_secs(10);
    let result = tokio::time::timeout(timeout, async {
        while let Some(msg_result) = receiver.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str::<WorkerMessage>(&text)
                        .map_err(|e| format!("invalid hello frame: {e}"));
                }
                Ok(Message::Close(_)) => {
                    return Err("connection closed before hello".to_string());
                }
                Ok(_) => continue,
                Err(e) => {
                    return Err(format!("receive error: {e}"));
                }
            }
        }
        Err("connection closed before hello".to_string())
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err("hello timeout (10s)".to_string()),
    }
}

/// Dispatch a parsed worker message to the appropriate handler.
async fn handle_worker_message(
    state: &AppState,
    connection_id: &str,
    worker_name: &str,
    registered_worker_id: &str,
    msg: WorkerMessage,
) {
    match msg {
        WorkerMessage::Hello { .. } => {
            tracing::warn!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                "duplicate hello message — ignoring"
            );
        }
        WorkerMessage::BuildAccepted { build_id } => {
            if !dispatch::authorize_lifecycle_message(
                &state.queue,
                &state.worker_senders,
                connection_id,
                build_id,
                cbsd_proto::ws::WorkerBuildAction::BuildAccepted,
            )
            .await
            {
                return;
            }
            dispatch::handle_build_accepted(state, connection_id, build_id.0).await;
        }
        WorkerMessage::BuildStarted { build_id } => {
            if !dispatch::authorize_lifecycle_message(
                &state.queue,
                &state.worker_senders,
                connection_id,
                build_id,
                cbsd_proto::ws::WorkerBuildAction::BuildStarted,
            )
            .await
            {
                return;
            }
            dispatch::handle_build_started(state, build_id.0).await;
        }
        WorkerMessage::BuildOutput {
            build_id,
            start_seq,
            ref lines,
        } => {
            tracing::debug!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                build_id = %build_id,
                start_seq = start_seq,
                line_count = lines.len(),
                "build output"
            );
            if !dispatch::authorize_lifecycle_message(
                &state.queue,
                &state.worker_senders,
                connection_id,
                build_id,
                cbsd_proto::ws::WorkerBuildAction::BuildOutput,
            )
            .await
            {
                return;
            }
            if let Err(e) = crate::logs::writer::write_build_output(
                &state.log_writer,
                &state.log_watchers,
                &state.config.log_dir,
                &state.pool,
                build_id.0,
                connection_id,
                start_seq,
                lines,
            )
            .await
            {
                tracing::error!(
                    build_id = %build_id,
                    "failed to write build output: {e}"
                );
            }
        }
        WorkerMessage::BuildFinished {
            build_id,
            status,
            ref error,
            ref build_report,
        } => {
            if !dispatch::authorize_lifecycle_message(
                &state.queue,
                &state.worker_senders,
                connection_id,
                build_id,
                cbsd_proto::ws::WorkerBuildAction::BuildFinished,
            )
            .await
            {
                return;
            }
            let status_str = match status {
                BuildFinishedStatus::Success => "success",
                BuildFinishedStatus::Failure => "failure",
                BuildFinishedStatus::Revoked => "revoked",
            };

            // Only pass the report for success builds.
            let report_json = if status == BuildFinishedStatus::Success {
                build_report
                    .as_ref()
                    .and_then(|v| serde_json::to_string(v).ok())
            } else {
                None
            };

            dispatch::handle_build_finished(
                state,
                connection_id,
                build_id.0,
                status_str,
                error.as_deref(),
                report_json.as_deref(),
            )
            .await;

            // Update last_seen on build_finished (proof-of-life).
            if let Err(e) = db::workers::update_last_seen(&state.pool, registered_worker_id).await {
                tracing::warn!(
                    worker_id = %registered_worker_id,
                    "failed to update last_seen on build_finished: {e}"
                );
            }
        }
        WorkerMessage::BuildRejected {
            build_id,
            ref reason,
        } => {
            if !dispatch::authorize_lifecycle_message(
                &state.queue,
                &state.worker_senders,
                connection_id,
                build_id,
                cbsd_proto::ws::WorkerBuildAction::BuildRejected,
            )
            .await
            {
                return;
            }
            dispatch::handle_build_rejected(state, connection_id, build_id.0, reason).await;
        }
        WorkerMessage::WorkerStatus {
            state: ws,
            build_id,
        } => {
            handle_worker_status(
                state,
                connection_id,
                worker_name,
                registered_worker_id,
                ws,
                build_id,
            )
            .await;
        }
        WorkerMessage::WorkerStopping { ref reason } => {
            tracing::info!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                reason = %reason,
                "worker stopping"
            );
            let mut queue = state.queue.lock().await;
            queue.set_worker_state(
                connection_id,
                WorkerState::Stopping {
                    registered_worker_id: registered_worker_id.to_string(),
                    worker_name: worker_name.to_string(),
                },
            );
        }
        WorkerMessage::Metrics { .. } => {
            // The wire contract exists (cbsd-proto), but the server does not yet
            // advertise `accepts_metrics`, so no compliant worker pushes this.
            // Ingestion under a server-stamped `worker` label lands in G5;
            // until then a stray sample is dropped rather than mis-attributed.
            tracing::debug!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                "received worker metrics before ingestion is wired; dropping"
            );
        }
    }
}

/// Snapshot of an active build observed during idle reconciliation. Captured
/// under the queue lock so the per-candidate DB lookup and queue mutation can
/// run without holding the lock across SQLite I/O (cbsd-rs CLAUDE.md
/// "Correctness Invariants" #2).
struct IdleReconcileCandidate {
    build_id: i64,
    prev_connection_id: String,
    receipt: ActiveAssignmentReceipt,
    /// `true` if the previous connection's worker is in the `Connected` state
    /// (still authenticated and live). `false` if Disconnected, Dead, or
    /// missing from the workers map.
    prev_connection_live: bool,
}

/// Outcome of the idle-reconcile per-candidate decision matrix (WCP D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleReconcileAction {
    /// Don't touch this build — leave it for normal in-flight resolution.
    /// Used for `dispatched + AwaitingReceipt` where the previous connection
    /// is still live (the dispatch might still complete).
    Skip,
    /// Roll the build back to QUEUED, clearing provenance.
    RollbackToQueued,
    /// Mark the build FAILURE — the worker reports idle but the DB says
    /// `started`, so the worker lost the executing build.
    FailBuild,
    /// Mark the build REVOKED — the worker reports idle but the DB says
    /// `revoking`. The revoke target is gone; finalise the build per
    /// WCP D3's `revoking + idle worker → revoked` row.
    MarkRevoked,
}

/// Pure-function decision matrix used by `(WorkerReportedState::Idle, _)`
/// after the cross-worker filter (build's persisted `worker_id` matches the
/// reporter). Per WCP D3:
/// - `dispatched + AwaitingReceipt + prev_live` → Skip
/// - `dispatched + AwaitingReceipt + !prev_live` → RollbackToQueued
/// - `dispatched + ReceivedByWorker` → RollbackToQueued (regardless of prev)
/// - `started` → FailBuild
/// - `revoking` → MarkRevoked
/// - other states → Skip
fn idle_reconcile_decision(
    db_state: &str,
    receipt: ActiveAssignmentReceipt,
    prev_connection_live: bool,
) -> IdleReconcileAction {
    match db_state {
        "dispatched" => match receipt {
            ActiveAssignmentReceipt::AwaitingReceipt if prev_connection_live => {
                IdleReconcileAction::Skip
            }
            _ => IdleReconcileAction::RollbackToQueued,
        },
        "started" => IdleReconcileAction::FailBuild,
        "revoking" => IdleReconcileAction::MarkRevoked,
        _ => IdleReconcileAction::Skip,
    }
}

/// Outcome of the audit-rem D12 dead-worker resolution table. Pure so each
/// row is unit-testable; [`resolve_dead_build`] maps it onto the existing
/// rollback / terminal-cleanup helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeadWorkerAction {
    /// `dispatched + AwaitingReceipt`: no owned message ever proved the worker
    /// held the assignment, so roll back to `queued` for redispatch.
    RollbackToQueued,
    /// Mark the build terminal `failure` with the given reason. Used for
    /// `dispatched + ReceivedByWorker` (the worker may have produced upstream
    /// side effects before dying, so failing avoids a duplicate-execution
    /// requeue) and for `started`.
    Fail(&'static str),
    /// `revoking`: the worker died mid-revoke; treat the revoke as complete.
    MarkRevoked(&'static str),
    /// Already terminal or an unexpected state — drop the stale active entry
    /// without a DB transition.
    RemoveOnly,
}

/// Pure-function resolution table for a worker declared dead (audit-rem D12),
/// over (`builds.state` × [`ActiveAssignmentReceipt`]):
/// - `dispatched + AwaitingReceipt` → RollbackToQueued
/// - `dispatched + ReceivedByWorker` → Fail (no requeue — possible side effects)
/// - `started` → Fail
/// - `revoking` → MarkRevoked
/// - other → RemoveOnly
///
/// Distinct from [`idle_reconcile_decision`]: there the worker is back and
/// reporting idle (so even `ReceivedByWorker` rolls back); here the worker is
/// gone, so `ReceivedByWorker` work-in-flight is failed rather than requeued.
fn dead_worker_resolution(db_state: &str, receipt: ActiveAssignmentReceipt) -> DeadWorkerAction {
    match db_state {
        "dispatched" => match receipt {
            ActiveAssignmentReceipt::AwaitingReceipt => DeadWorkerAction::RollbackToQueued,
            ActiveAssignmentReceipt::ReceivedByWorker => {
                DeadWorkerAction::Fail("worker died after accepting assignment")
            }
        },
        "started" => DeadWorkerAction::Fail("worker died during execution"),
        "revoking" => DeadWorkerAction::MarkRevoked("revoke completed by worker death"),
        _ => DeadWorkerAction::RemoveOnly,
    }
}

/// Terminal status accepted by [`cleanup_terminal_state`]. Closed set,
/// kept narrow so the helper cannot be invoked with a non-terminal
/// state string like `"queued"` or `"dispatched"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalStatus {
    /// `builds.state = 'failure'`.
    Failure,
    /// `builds.state = 'revoked'`.
    Revoked,
}

impl TerminalStatus {
    /// Wire-format spelling stored in the `builds.state` column.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Failure => "failure",
            Self::Revoked => "revoked",
        }
    }
}

/// Shared terminal-state cleanup used by every path that finalises an
/// active build (idle reconciler's FailBuild and MarkRevoked branches, the
/// dead-worker resolver's Fail and MarkRevoked actions, and the dispatch
/// module's integrity-reject and revoke-timeout paths). Marks the build
/// finished with the supplied [`TerminalStatus`], finalises the build
/// log row, and removes the active entry and log watcher. Takes the
/// three state pieces it touches explicitly so integration tests can
/// drive it without a full `AppState`.
pub(crate) async fn cleanup_terminal_state(
    pool: &SqlitePool,
    queue: &SharedBuildQueue,
    log_watchers: &LogWatchers,
    build_id: i64,
    status: TerminalStatus,
    reason: &str,
) {
    let status_str = status.as_str();
    if let Err(e) =
        crate::db::builds::set_build_finished(pool, build_id, status_str, Some(reason), None).await
    {
        tracing::error!(
            build_id,
            status = status_str,
            "failed to mark build {status_str}: {e}"
        );
    }
    if let Err(e) = crate::db::builds::set_build_log_finished(pool, build_id).await {
        tracing::error!(build_id, "failed to mark log finished: {e}");
    }
    {
        let mut q = queue.lock().await;
        q.active.remove(&build_id);
    }
    {
        let mut watchers = log_watchers.lock().await;
        watchers.remove(&build_id);
    }
}

/// Per-candidate idle-reconcile body. Runs the DB-backed cross-worker
/// filter (WCP D3 / gap G8) and dispatches on `idle_reconcile_decision`.
/// Takes the state pieces it touches explicitly so integration tests can
/// drive it with a synthetic candidate, without a full `AppState`.
async fn idle_reconcile_one(
    pool: &SqlitePool,
    queue: &SharedBuildQueue,
    log_watchers: &LogWatchers,
    candidate: &IdleReconcileCandidate,
    registered_worker_id: &str,
) {
    let build = match crate::db::builds::get_build(pool, candidate.build_id).await {
        Ok(Some(b)) => b,
        _ => return,
    };

    // Cross-worker filter: only touch builds whose persisted worker_id
    // matches the reporter's authenticated id.
    if build.worker_id.as_deref() != Some(registered_worker_id) {
        return;
    }

    match idle_reconcile_decision(
        &build.state,
        candidate.receipt,
        candidate.prev_connection_live,
    ) {
        IdleReconcileAction::Skip => {
            tracing::debug!(
                build_id = candidate.build_id,
                prev_connection = %candidate.prev_connection_id,
                db_state = %build.state,
                receipt = ?candidate.receipt,
                "reconnect idle: skip"
            );
        }
        IdleReconcileAction::RollbackToQueued => {
            tracing::warn!(
                build_id = candidate.build_id,
                prev_connection = %candidate.prev_connection_id,
                receipt = ?candidate.receipt,
                "reconnect idle: rolling back stale dispatch to queued"
            );
            crate::metrics::lifecycle::record_requeue("reconnect_stale");
            dispatch::rollback_active_to_queued(pool, queue, log_watchers, candidate.build_id)
                .await;
        }
        IdleReconcileAction::FailBuild => {
            tracing::error!(
                build_id = candidate.build_id,
                prev_connection = %candidate.prev_connection_id,
                "reconnect idle: DB=started — marking FAILURE(worker lost build)"
            );
            cleanup_terminal_state(
                pool,
                queue,
                log_watchers,
                candidate.build_id,
                TerminalStatus::Failure,
                "worker lost build",
            )
            .await;
        }
        IdleReconcileAction::MarkRevoked => {
            tracing::warn!(
                build_id = candidate.build_id,
                prev_connection = %candidate.prev_connection_id,
                "reconnect idle: DB=revoking — marking REVOKED"
            );
            cleanup_terminal_state(
                pool,
                queue,
                log_watchers,
                candidate.build_id,
                TerminalStatus::Revoked,
                "idle worker on revoking build",
            )
            .await;
        }
    }
}

/// Reconnection decision table.
async fn handle_worker_status(
    state: &AppState,
    connection_id: &str,
    worker_name: &str,
    registered_worker_id: &str,
    reported_state: WorkerReportedState,
    reported_build_id: Option<BuildId>,
) {
    tracing::info!(
        connection_id = %connection_id,
        worker_name = %worker_name,
        reported_state = ?reported_state,
        reported_build_id = ?reported_build_id,
        "processing worker status"
    );

    match (reported_state, reported_build_id) {
        (WorkerReportedState::Building, Some(build_id)) => {
            let build = match crate::db::builds::get_build(&state.pool, build_id.0).await {
                Ok(Some(b)) => b,
                Ok(None) => {
                    tracing::warn!(
                        build_id = build_id.0,
                        "reconnect: build not found in DB — sending revoke"
                    );
                    dispatch::send_reporter_directed_revoke(
                        &state.worker_senders,
                        connection_id,
                        build_id,
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    tracing::error!(build_id = build_id.0, "reconnect: DB lookup failed: {e}");
                    return;
                }
            };
            let db_state = build.state.as_str();
            let db_worker_id = build.worker_id.as_deref();

            // Two-phase ownership check (WCP D1, migration step 4-5): for
            // resumable states, the persisted `builds.worker_id` must match
            // the authenticated registered worker ID of the reporter. The
            // DB query above already ran outside the queue lock; the queue
            // mutation below reacquires the lock.
            let resumable = matches!(db_state, "dispatched" | "started");
            let owns = resumable
                && db_worker_id
                    .map(|w| w == registered_worker_id)
                    .unwrap_or(false);

            if resumable && !owns {
                tracing::warn!(
                    build_id = build_id.0,
                    db_state = %db_state,
                    db_worker_id = ?db_worker_id,
                    registered_worker_id = %registered_worker_id,
                    "reconnect: building claim for build not assigned to this worker"
                );
                dispatch::send_unauthorized_action(
                    &state.worker_senders,
                    connection_id,
                    build_id,
                    cbsd_proto::ws::WorkerBuildAction::WorkerStatus,
                    cbsd_proto::ws::UnauthorizedBuildReason::NotAssigned,
                )
                .await;
                dispatch::send_reporter_directed_revoke(
                    &state.worker_senders,
                    connection_id,
                    build_id,
                )
                .await;
                return;
            }

            match db_state {
                "queued" => {
                    tracing::warn!(
                        build_id = build_id.0,
                        "reconnect: DB=queued but worker building — sending revoke"
                    );
                    let _ = dispatch::send_build_revoke(state, build_id.0).await;
                }
                "dispatched" => {
                    tracing::info!(
                        build_id = build_id.0,
                        "reconnect: DB=dispatched, worker building — accepted-phase \
                         reconnect: mark received, keep dispatched (audit-rem D11)"
                    );
                    // audit-rem D11: the worker is in the accepted phase — it
                    // reports Building but has not sent build_started. Treat
                    // this as an authoritative receipt of build_accepted (which
                    // may have been lost in the disconnect): mark the receipt
                    // ReceivedByWorker and cancel the dispatch-ack timer so it
                    // cannot requeue a build the worker is already running
                    // (double execution). SM-S stays `dispatched` until the
                    // worker's subprocess sends build_started.
                    dispatch::attach_connection_and_mark_received(
                        &state.queue,
                        build_id.0,
                        connection_id,
                    )
                    .await;
                }
                "started" => {
                    tracing::info!(
                        build_id = build_id.0,
                        "reconnect: DB=started, worker building — resume"
                    );
                    let mut queue = state.queue.lock().await;
                    if let Some(ab) = queue.active.get_mut(&build_id.0) {
                        ab.connection_id = connection_id.to_string();
                    }
                }
                "revoking" => {
                    tracing::info!(
                        build_id = build_id.0,
                        "reconnect: DB=revoking, worker building — re-sending revoke"
                    );
                    let _ = dispatch::send_build_revoke(state, build_id.0).await;
                }
                "failure" | "success" | "revoked" => {
                    tracing::warn!(
                        build_id = build_id.0,
                        db_state = %db_state,
                        "reconnect: terminal state but worker building — sending revoke"
                    );
                    let _ = dispatch::send_build_revoke(state, build_id.0).await;
                }
                other => {
                    tracing::warn!(
                        build_id = build_id.0,
                        db_state = %other,
                        "reconnect: unexpected DB state"
                    );
                }
            }
        }

        (WorkerReportedState::Idle, _) => {
            // Phase 1 (under queue lock): snapshot every active entry whose
            // connection differs from the reporter, capturing the prior
            // connection's liveness and the receipt state. Note the active
            // entry's connection_id is the PRIOR connection; the reporter
            // is on a NEW connection_id.
            let candidates: Vec<IdleReconcileCandidate> = {
                let queue = state.queue.lock().await;
                queue
                    .active
                    .values()
                    .filter(|ab| ab.connection_id != connection_id)
                    .map(|ab| IdleReconcileCandidate {
                        build_id: ab.build_id,
                        prev_connection_id: ab.connection_id.clone(),
                        receipt: ab.receipt,
                        prev_connection_live: queue
                            .get_worker(&ab.connection_id)
                            .is_some_and(|ws| matches!(ws, WorkerState::Connected { .. })),
                    })
                    .collect()
            };

            // Phase 2 (no lock held): for each candidate, run the
            // DB-backed ownership check (per WCP D3 / gap G8 fix) and
            // dispatch. Extracted to `idle_reconcile_one` so integration
            // tests can drive the same path with explicit state args.
            for cand in &candidates {
                idle_reconcile_one(
                    &state.pool,
                    &state.queue,
                    &state.log_watchers,
                    cand,
                    registered_worker_id,
                )
                .await;
            }

            if let Err(dispatch::DispatchError::NothingToDispatch) =
                dispatch::try_dispatch(state).await
            {
                tracing::debug!(
                    connection_id = %connection_id,
                    "reconnect idle: no pending builds"
                );
            }
        }

        (WorkerReportedState::Building, None) => {
            tracing::warn!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                "worker reports building but no build_id — ignoring"
            );
        }
    }
}

/// Handle a worker transitioning to Dead (grace period expired). Resolves
/// every active build owned by that connection via the audit-rem D12
/// resolution table ([`dead_worker_resolution`]), then attempts a redispatch
/// for anything rolled back to `queued`.
pub async fn handle_worker_dead(state: &AppState, connection_id: &str) {
    // Snapshot (build_id, receipt) under the queue lock. The worker is gone,
    // so the receipt cannot change between this snapshot and resolution.
    let owned = {
        let queue = state.queue.lock().await;
        queue.active_builds_with_receipt_for_connection(connection_id)
    };

    for (build_id, receipt) in owned {
        resolve_dead_build(
            &state.pool,
            &state.queue,
            &state.log_watchers,
            connection_id,
            build_id,
            receipt,
        )
        .await;
    }

    // Anything rolled back to `queued` above needs another idle worker. A
    // single post-loop call is functionally equivalent to the previous
    // per-build dispatch.
    if let Err(dispatch::DispatchError::NothingToDispatch) = dispatch::try_dispatch(state).await {
        tracing::debug!(
            connection_id = %connection_id,
            "worker dead: no re-dispatch needed"
        );
    }
}

/// Resolve a single dead-worker-owned build per the audit-rem D12 table.
/// Reads the current DB state, applies [`dead_worker_resolution`] against the
/// in-memory receipt, and executes the resulting action via the existing
/// rollback / terminal-cleanup helpers. Takes explicit state pieces (not
/// `&AppState`) so integration tests can drive each row directly.
async fn resolve_dead_build(
    pool: &SqlitePool,
    queue: &SharedBuildQueue,
    log_watchers: &LogWatchers,
    connection_id: &str,
    build_id: i64,
    receipt: ActiveAssignmentReceipt,
) {
    let db_state = match crate::db::builds::get_build(pool, build_id).await {
        Ok(Some(b)) => b.state,
        Ok(None) => return,
        Err(e) => {
            tracing::error!(build_id, "dead worker DB lookup failed: {e}");
            return;
        }
    };

    match dead_worker_resolution(&db_state, receipt) {
        DeadWorkerAction::RollbackToQueued => {
            tracing::warn!(
                build_id,
                connection_id = %connection_id,
                "worker dead — rolling unacknowledged dispatch back to queued"
            );
            crate::metrics::lifecycle::record_requeue("worker_dead");
            dispatch::rollback_active_to_queued(pool, queue, log_watchers, build_id).await;
        }
        DeadWorkerAction::Fail(reason) => {
            tracing::error!(
                build_id,
                connection_id = %connection_id,
                reason,
                "worker dead — marking build failure"
            );
            cleanup_terminal_state(
                pool,
                queue,
                log_watchers,
                build_id,
                TerminalStatus::Failure,
                reason,
            )
            .await;
        }
        DeadWorkerAction::MarkRevoked(reason) => {
            tracing::warn!(
                build_id,
                connection_id = %connection_id,
                "worker dead while revoking — marking revoked"
            );
            cleanup_terminal_state(
                pool,
                queue,
                log_watchers,
                build_id,
                TerminalStatus::Revoked,
                reason,
            )
            .await;
        }
        DeadWorkerAction::RemoveOnly => {
            let mut q = queue.lock().await;
            q.active.remove(&build_id);
        }
    }
}

/// Clean up worker state on connection close.
async fn cleanup_worker(
    state: &AppState,
    connection_id: &str,
    worker_name: &str,
    registered_worker_id: &str,
    is_stopping: bool,
) {
    // Remove the outbound sender channel.
    {
        let mut senders = state.worker_senders.lock().await;
        senders.remove(connection_id);
    }

    if is_stopping {
        tracing::info!(
            connection_id = %connection_id,
            worker_name = %worker_name,
            "worker stopped — marking dead (immediate)"
        );

        {
            let mut queue = state.queue.lock().await;
            queue.set_worker_state(connection_id, WorkerState::Dead);
        }

        handle_worker_dead(state, connection_id).await;
    } else {
        let arch = {
            let queue = state.queue.lock().await;
            queue.get_worker(connection_id).and_then(|w| w.arch())
        };

        if let Some(arch) = arch {
            tracing::warn!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                "worker disconnected — entering grace period"
            );

            {
                let mut queue = state.queue.lock().await;
                queue.set_worker_state(
                    connection_id,
                    WorkerState::Disconnected {
                        since: tokio::time::Instant::now(),
                        registered_worker_id: registered_worker_id.to_string(),
                        worker_name: worker_name.to_string(),
                        arch,
                    },
                );
            }

            let grace_secs = state.config.timeouts.liveness_grace_period_secs;
            crate::ws::liveness::start_grace_period_monitor(state, connection_id, grace_secs);
        } else {
            tracing::warn!(
                connection_id = %connection_id,
                worker_name = %worker_name,
                "worker disconnected with unknown arch — marking dead"
            );

            {
                let mut queue = state.queue.lock().await;
                queue.set_worker_state(connection_id, WorkerState::Dead);
            }

            handle_worker_dead(state, connection_id).await;
        }
    }
}

/// Send a JSON-serialized server message over the WebSocket.
async fn send_json(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    msg: &ServerMessage,
) -> Result<(), axum::Error> {
    use futures_util::SinkExt;
    let text = serde_json::to_string(msg).expect("ServerMessage serialization cannot fail");
    sender.send(Message::Text(text.into())).await
}

/// Return a short type name for a `WorkerMessage` (for error messages).
fn message_type_name(msg: &WorkerMessage) -> &'static str {
    match msg {
        WorkerMessage::Hello { .. } => "hello",
        WorkerMessage::BuildAccepted { .. } => "build_accepted",
        WorkerMessage::BuildStarted { .. } => "build_started",
        WorkerMessage::BuildOutput { .. } => "build_output",
        WorkerMessage::BuildFinished { .. } => "build_finished",
        WorkerMessage::BuildRejected { .. } => "build_rejected",
        WorkerMessage::WorkerStatus { .. } => "worker_status",
        WorkerMessage::WorkerStopping { .. } => "worker_stopping",
        WorkerMessage::Metrics { .. } => "metrics",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_dispatched_awaiting_receipt_live_prev_skips() {
        let action = idle_reconcile_decision(
            "dispatched",
            ActiveAssignmentReceipt::AwaitingReceipt,
            /* prev_connection_live */ true,
        );
        assert_eq!(action, IdleReconcileAction::Skip);
    }

    #[test]
    fn idle_dispatched_awaiting_receipt_dead_prev_rolls_back() {
        let action = idle_reconcile_decision(
            "dispatched",
            ActiveAssignmentReceipt::AwaitingReceipt,
            /* prev_connection_live */ false,
        );
        assert_eq!(action, IdleReconcileAction::RollbackToQueued);
    }

    #[test]
    fn idle_dispatched_received_by_worker_always_rolls_back() {
        // Live previous connection is no protection here: ReceivedByWorker
        // means the worker had it and now reports idle, so it has dropped
        // the build (WCP D3).
        for prev_live in [true, false] {
            let action = idle_reconcile_decision(
                "dispatched",
                ActiveAssignmentReceipt::ReceivedByWorker,
                prev_live,
            );
            assert_eq!(
                action,
                IdleReconcileAction::RollbackToQueued,
                "prev_live={prev_live}"
            );
        }
    }

    #[test]
    fn idle_started_fails_the_build() {
        for receipt in [
            ActiveAssignmentReceipt::AwaitingReceipt,
            ActiveAssignmentReceipt::ReceivedByWorker,
        ] {
            for prev_live in [true, false] {
                let action = idle_reconcile_decision("started", receipt, prev_live);
                assert_eq!(
                    action,
                    IdleReconcileAction::FailBuild,
                    "receipt={receipt:?} prev_live={prev_live}"
                );
            }
        }
    }

    #[test]
    fn idle_other_states_are_skipped() {
        for state in ["queued", "failure", "success", "revoked"] {
            let action =
                idle_reconcile_decision(state, ActiveAssignmentReceipt::ReceivedByWorker, false);
            assert_eq!(action, IdleReconcileAction::Skip, "state={state}");
        }
    }

    #[test]
    fn idle_revoking_marks_revoked() {
        // WCP D3 matrix row: revoking + idle worker → revoked. Receipt
        // state and prev-connection liveness are irrelevant for this row.
        for receipt in [
            ActiveAssignmentReceipt::AwaitingReceipt,
            ActiveAssignmentReceipt::ReceivedByWorker,
        ] {
            for prev_live in [true, false] {
                let action = idle_reconcile_decision("revoking", receipt, prev_live);
                assert_eq!(
                    action,
                    IdleReconcileAction::MarkRevoked,
                    "receipt={receipt:?} prev_live={prev_live}"
                );
            }
        }
    }

    // ---- Integration tests for `idle_reconcile_one` (F4) ----
    //
    // These exercise the per-candidate idle reconciler against a real
    // SQLite pool and a real `SharedBuildQueue` / `LogWatchers` map. They
    // verify the DB-side observable state — column resets, terminal
    // status transitions — that the pure-function `idle_reconcile_decision`
    // tests above cannot observe.

    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use cbsd_proto::{
        Arch, BuildComponent, BuildDescriptor, BuildDestImage, BuildSignedOffBy, BuildTarget,
        Priority,
    };
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tokio::sync::Mutex;

    use crate::queue::{ActiveBuild, BuildQueue};

    async fn test_pool() -> SqlitePool {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let url = format!(
            "file:idle_reconcile_test_{pid}_{id}?mode=memory&cache=shared",
            pid = std::process::id(),
        );
        let options = SqliteConnectOptions::from_str(&url)
            .expect("valid sqlite URL")
            .pragma("foreign_keys", "ON")
            .pragma("busy_timeout", "5000");
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
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

    fn sample_descriptor() -> BuildDescriptor {
        BuildDescriptor {
            version: "v".into(),
            channel: None,
            version_type: None,
            signed_off_by: BuildSignedOffBy {
                user: "u".into(),
                email: "u@e.com".into(),
            },
            dst_image: BuildDestImage {
                name: "img".into(),
                tag: "tag".into(),
            },
            components: vec![BuildComponent {
                name: "c".into(),
                git_ref: "main".into(),
                repo: None,
            }],
            build: BuildTarget {
                distro: "fedora".into(),
                os_version: "42".into(),
                artifact_type: "rpm".into(),
                arch: Arch::X86_64,
            },
        }
    }

    async fn seed_user(pool: &SqlitePool, email: &str) {
        sqlx::query!(
            "INSERT INTO users (email, name, active, is_robot) VALUES (?, ?, 1, 0)",
            email,
            email,
        )
        .execute(pool)
        .await
        .expect("seed user");
    }

    async fn insert_in_state(pool: &SqlitePool, state: &str, worker_id: &str) -> i64 {
        seed_user(pool, "u@e.com").await;
        let id = crate::db::builds::insert_build(
            pool,
            r#"{"name":"t"}"#,
            "u@e.com",
            "normal",
            None,
            None,
            None,
        )
        .await
        .expect("insert");
        sqlx::query!(
            "UPDATE builds SET state = ?, worker_id = ?, trace_id = 't' WHERE id = ?",
            state,
            worker_id,
            id,
        )
        .execute(pool)
        .await
        .expect("set state");
        // build_logs row required for set_build_log_finished.
        crate::db::builds::insert_build_log_row(pool, id, "/tmp/x.log")
            .await
            .expect("insert build_log");
        id
    }

    fn empty_queue() -> SharedBuildQueue {
        Arc::new(Mutex::new(BuildQueue::new()))
    }

    fn empty_watchers() -> LogWatchers {
        Arc::new(Mutex::new(HashMap::new()))
    }

    fn make_active(build_id: i64, connection_id: &str) -> ActiveBuild {
        ActiveBuild {
            build_id,
            connection_id: connection_id.to_string(),
            dispatched_at: tokio::time::Instant::now(),
            trace_id: "t".to_string(),
            descriptor: sample_descriptor(),
            priority: Priority::Normal,
            ack_cancel: tokio_util::sync::CancellationToken::new(),
            receipt: ActiveAssignmentReceipt::AwaitingReceipt,
        }
    }

    #[tokio::test]
    async fn idle_reconcile_one_dispatched_awaiting_dead_prev_rolls_back_and_clears_db() {
        let pool = test_pool().await;
        let queue = empty_queue();
        let watchers = empty_watchers();
        let build_id = insert_in_state(&pool, "dispatched", "worker-1").await;

        {
            let mut q = queue.lock().await;
            q.active.insert(build_id, make_active(build_id, "old-conn"));
        }

        let candidate = IdleReconcileCandidate {
            build_id,
            prev_connection_id: "old-conn".to_string(),
            receipt: ActiveAssignmentReceipt::AwaitingReceipt,
            prev_connection_live: false, // dead/disconnected prev
        };

        idle_reconcile_one(&pool, &queue, &watchers, &candidate, "worker-1").await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(build.state, "queued", "must roll back to queued");
        assert!(build.worker_id.is_none());
        assert!(build.trace_id.is_none());
        assert!(build.error.is_none());
        assert!(build.started_at.is_none());
        assert!(build.finished_at.is_none());
        assert!(build.build_report.is_none());
    }

    #[tokio::test]
    async fn idle_reconcile_one_revoking_marks_revoked() {
        let pool = test_pool().await;
        let queue = empty_queue();
        let watchers = empty_watchers();
        let build_id = insert_in_state(&pool, "revoking", "worker-1").await;

        {
            let mut q = queue.lock().await;
            q.active.insert(build_id, make_active(build_id, "old-conn"));
        }
        {
            let (tx, _rx) = tokio::sync::watch::channel(());
            watchers.lock().await.insert(build_id, tx);
        }

        let candidate = IdleReconcileCandidate {
            build_id,
            prev_connection_id: "old-conn".to_string(),
            receipt: ActiveAssignmentReceipt::ReceivedByWorker,
            prev_connection_live: true,
        };

        idle_reconcile_one(&pool, &queue, &watchers, &candidate, "worker-1").await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(build.state, "revoked", "must finalise to revoked");
        assert!(build.finished_at.is_some());

        // Verify build_logs.finished is set — this is what the
        // `set_build_log_finished` call inside `cleanup_terminal_state`
        // does, and the v1/v2 review N2 specifically called out that
        // this assertion was missing.
        let log_row = sqlx::query!(
            "SELECT finished FROM build_logs WHERE build_id = ?",
            build_id,
        )
        .fetch_one(&pool)
        .await
        .expect("log row");
        assert_eq!(
            log_row.finished, 1,
            "build_logs.finished must be set when MarkRevoked completes"
        );

        assert!(
            !queue.lock().await.active.contains_key(&build_id),
            "active entry must be removed"
        );
        assert!(
            !watchers.lock().await.contains_key(&build_id),
            "log watcher must be removed"
        );
    }

    #[tokio::test]
    async fn idle_reconcile_one_skips_when_db_worker_id_mismatch() {
        // Cross-worker filter (gap G8): reporter is `worker-1` but the
        // DB row says the build belongs to `worker-2`. Must skip.
        let pool = test_pool().await;
        let queue = empty_queue();
        let watchers = empty_watchers();
        let build_id = insert_in_state(&pool, "dispatched", "worker-2").await;

        {
            let mut q = queue.lock().await;
            q.active.insert(build_id, make_active(build_id, "old-conn"));
        }

        let candidate = IdleReconcileCandidate {
            build_id,
            prev_connection_id: "old-conn".to_string(),
            receipt: ActiveAssignmentReceipt::ReceivedByWorker,
            prev_connection_live: false,
        };

        idle_reconcile_one(&pool, &queue, &watchers, &candidate, "worker-1").await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(
            build.state, "dispatched",
            "state must not change when reporter does not own the build"
        );
        assert_eq!(build.worker_id.as_deref(), Some("worker-2"));
    }

    /// Pins the contract relied on by `dispatch::handle_build_rejected`
    /// integrity-failure arm and `dispatch::handle_revoke_timeout` after
    /// they were migrated to delegate to `cleanup_terminal_state`. Both
    /// callsites previously inlined the cleanup; the integrity arm in
    /// particular forgot `set_build_log_finished`, which kept SSE log
    /// streams hanging (review v3 finding NA2). This test ensures every
    /// future invocation of `cleanup_terminal_state` on the "failure"
    /// path also flips `build_logs.finished = 1`.
    #[tokio::test]
    async fn cleanup_terminal_state_failure_path_finalises_build_log() {
        let pool = test_pool().await;
        let queue = empty_queue();
        let watchers = empty_watchers();
        let build_id = insert_in_state(&pool, "started", "worker-1").await;

        {
            let mut q = queue.lock().await;
            q.active.insert(build_id, make_active(build_id, "conn-1"));
        }
        {
            let (tx, _rx) = tokio::sync::watch::channel(());
            watchers.lock().await.insert(build_id, tx);
        }

        cleanup_terminal_state(
            &pool,
            &queue,
            &watchers,
            build_id,
            TerminalStatus::Failure,
            "integrity rejected",
        )
        .await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(build.state, "failure");

        let log_row = sqlx::query!(
            "SELECT finished FROM build_logs WHERE build_id = ?",
            build_id,
        )
        .fetch_one(&pool)
        .await
        .expect("log row");
        assert_eq!(
            log_row.finished, 1,
            "build_logs.finished must be set so SSE streams unblock"
        );
        assert!(
            !queue.lock().await.active.contains_key(&build_id),
            "active entry must be removed"
        );
        assert!(
            !watchers.lock().await.contains_key(&build_id),
            "log watcher must be removed"
        );
    }

    // -- audit-rem D12: dead-worker resolution table --

    #[test]
    fn dead_worker_dispatched_awaiting_rolls_back() {
        assert_eq!(
            dead_worker_resolution("dispatched", ActiveAssignmentReceipt::AwaitingReceipt),
            DeadWorkerAction::RollbackToQueued
        );
    }

    #[test]
    fn dead_worker_dispatched_received_fails() {
        assert_eq!(
            dead_worker_resolution("dispatched", ActiveAssignmentReceipt::ReceivedByWorker),
            DeadWorkerAction::Fail("worker died after accepting assignment")
        );
    }

    #[test]
    fn dead_worker_started_fails_regardless_of_receipt() {
        for receipt in [
            ActiveAssignmentReceipt::AwaitingReceipt,
            ActiveAssignmentReceipt::ReceivedByWorker,
        ] {
            assert_eq!(
                dead_worker_resolution("started", receipt),
                DeadWorkerAction::Fail("worker died during execution")
            );
        }
    }

    #[test]
    fn dead_worker_revoking_marks_revoked_regardless_of_receipt() {
        for receipt in [
            ActiveAssignmentReceipt::AwaitingReceipt,
            ActiveAssignmentReceipt::ReceivedByWorker,
        ] {
            assert_eq!(
                dead_worker_resolution("revoking", receipt),
                DeadWorkerAction::MarkRevoked("revoke completed by worker death")
            );
        }
    }

    #[test]
    fn dead_worker_terminal_or_queued_removes_only() {
        for state in ["success", "failure", "revoked", "queued"] {
            assert_eq!(
                dead_worker_resolution(state, ActiveAssignmentReceipt::ReceivedByWorker),
                DeadWorkerAction::RemoveOnly
            );
        }
    }

    /// Insert a build in `state` with an active entry carrying `receipt`, then
    /// run the dead-worker resolver against it. Returns (pool, queue, id).
    async fn run_resolve_dead(
        state: &str,
        receipt: ActiveAssignmentReceipt,
    ) -> (SqlitePool, SharedBuildQueue, i64) {
        let pool = test_pool().await;
        let queue = empty_queue();
        let watchers = empty_watchers();
        let build_id = insert_in_state(&pool, state, "worker-1").await;
        {
            let mut q = queue.lock().await;
            q.active.insert(
                build_id,
                ActiveBuild {
                    receipt,
                    ..make_active(build_id, "old-conn")
                },
            );
        }
        resolve_dead_build(&pool, &queue, &watchers, "old-conn", build_id, receipt).await;
        (pool, queue, build_id)
    }

    #[tokio::test]
    async fn resolve_dead_dispatched_awaiting_rolls_back_to_queued() {
        let (pool, queue, build_id) =
            run_resolve_dead("dispatched", ActiveAssignmentReceipt::AwaitingReceipt).await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(
            build.state, "queued",
            "AwaitingReceipt → rollback to queued"
        );
        let q = queue.lock().await;
        assert!(!q.active.contains_key(&build_id), "active entry removed");
        assert!(
            q.contains(BuildId(build_id)),
            "rolled-back build must be re-enqueued"
        );
    }

    #[tokio::test]
    async fn resolve_dead_dispatched_received_fails_and_does_not_requeue() {
        let (pool, queue, build_id) =
            run_resolve_dead("dispatched", ActiveAssignmentReceipt::ReceivedByWorker).await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(build.state, "failure", "ReceivedByWorker → failure");
        assert_eq!(
            build.error.as_deref(),
            Some("worker died after accepting assignment")
        );
        let q = queue.lock().await;
        assert!(!q.active.contains_key(&build_id), "active entry removed");
        assert!(
            !q.contains(BuildId(build_id)),
            "must NOT requeue — work may have side-effected (double-exec guard)"
        );
    }

    #[tokio::test]
    async fn resolve_dead_started_marks_failure() {
        let (pool, _queue, build_id) =
            run_resolve_dead("started", ActiveAssignmentReceipt::ReceivedByWorker).await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(build.state, "failure");
        assert_eq!(build.error.as_deref(), Some("worker died during execution"));
    }

    #[tokio::test]
    async fn resolve_dead_revoking_marks_revoked() {
        let (pool, _queue, build_id) =
            run_resolve_dead("revoking", ActiveAssignmentReceipt::AwaitingReceipt).await;

        let build = crate::db::builds::get_build(&pool, build_id)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(build.state, "revoked");
        assert_eq!(
            build.error.as_deref(),
            Some("revoke completed by worker death")
        );
    }
}
