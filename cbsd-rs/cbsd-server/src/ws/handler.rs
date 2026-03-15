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

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

use cbsd_proto::ws::{BuildFinishedStatus, ServerMessage, WorkerMessage, WorkerReportedState};
use cbsd_proto::BuildId;

use crate::app::AppState;
use crate::ws::dispatch;
use crate::ws::liveness::WorkerState;

/// HTTP upgrade handler for `GET /ws/worker`.
///
/// Auth is performed manually from the upgrade request headers because the
/// `AuthUser` extractor targets REST endpoints, not WebSocket upgrades.
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

    crate::auth::api_keys::verify_api_key(&state.pool, &state.api_key_cache, token)
        .await
        .map_err(|e| {
            tracing::warn!("ws upgrade rejected: {e}");
            StatusCode::UNAUTHORIZED
        })?;

    // Generate a server-assigned connection UUID
    let connection_id = uuid::Uuid::new_v4().to_string();
    tracing::info!(connection_id = %connection_id, "ws upgrade accepted");

    Ok(ws.on_upgrade(move |socket| handle_connection(socket, state, connection_id)))
}

/// Main per-connection loop. Runs until the WebSocket closes.
async fn handle_connection(socket: WebSocket, state: AppState, connection_id: String) {
    let (mut sender, mut receiver) = socket.split();

    // Step 1: Wait for the hello message (first text frame).
    let hello = match wait_for_hello(&mut receiver).await {
        Ok(h) => h,
        Err(reason) => {
            tracing::warn!(
                connection_id = %connection_id,
                "ws handshake failed: {reason}"
            );
            let _ = send_json(
                &mut sender,
                &ServerMessage::Error {
                    reason,
                    min_version: Some(1),
                    max_version: Some(1),
                },
            )
            .await;
            return;
        }
    };

    // Step 2: Validate protocol version.
    let (worker_id, arch, cores_total, ram_total_mb) = match hello {
        WorkerMessage::Hello {
            protocol_version,
            worker_id,
            arch,
            cores_total,
            ram_total_mb,
        } => {
            if protocol_version != 1 {
                let reason = format!(
                    "unsupported protocol version {protocol_version}; server supports 1"
                );
                tracing::warn!(
                    connection_id = %connection_id,
                    worker_id = %worker_id,
                    "{reason}"
                );
                let _ = send_json(
                    &mut sender,
                    &ServerMessage::Error {
                        reason,
                        min_version: Some(1),
                        max_version: Some(1),
                    },
                )
                .await;
                return;
            }
            (worker_id, arch, cores_total, ram_total_mb)
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

    tracing::info!(
        connection_id = %connection_id,
        worker_id = %worker_id,
        arch = %arch,
        cores = cores_total,
        ram_mb = ram_total_mb,
        "worker connected"
    );

    // Step 3: Register worker in the build queue.
    {
        let mut queue = state.queue.lock().await;
        queue.register_worker(
            connection_id.clone(),
            WorkerState::Connected {
                worker_id: worker_id.clone(),
                arch,
                cores_total,
                ram_total_mb,
            },
        );
    }

    // Step 3b: Register an outbound message channel for this worker.
    // The dispatch engine sends messages via this channel; the forwarding
    // task below reads from it and writes to the actual WebSocket.
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
            protocol_version: 1,
            connection_id: connection_id.clone(),
            grace_period_secs,
        },
    )
    .await
    {
        tracing::error!(
            connection_id = %connection_id,
            "failed to send welcome: {e}"
        );
        cleanup_worker(&state, &connection_id, &worker_id, false).await;
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
    //
    // We run two concurrent tasks:
    // - Forwarding: reads from outbound_rx and writes to the WebSocket sender.
    // - Receiving: reads from the WebSocket receiver and dispatches messages.
    //
    // When either task ends, we cancel the other.
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
                        worker_id = %worker_id,
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
                                &worker_id,
                                worker_msg,
                            )
                            .await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                connection_id = %connection_id,
                                worker_id = %worker_id,
                                "failed to parse worker message: {e}"
                            );
                        }
                    }
                }
                Message::Close(_) => {
                    tracing::info!(
                        connection_id = %connection_id,
                        worker_id = %worker_id,
                        "ws close frame received"
                    );
                    break;
                }
                // Ping/Pong handled automatically by axum; binary frames ignored.
                _ => {}
            }
        }
    };

    // Run both tasks concurrently; when one finishes the other is dropped.
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
    cleanup_worker(&state, &connection_id, &worker_id, is_stopping).await;
}

/// Wait for the first text frame and parse it as a `WorkerMessage`.
async fn wait_for_hello(
    receiver: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Result<WorkerMessage, String> {
    use futures_util::StreamExt;

    // Give the worker 10 seconds to send hello.
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
                Ok(_) => {
                    // Skip ping/pong/binary
                    continue;
                }
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
    worker_id: &str,
    msg: WorkerMessage,
) {
    match msg {
        WorkerMessage::Hello { .. } => {
            tracing::warn!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                "duplicate hello message — ignoring"
            );
        }
        WorkerMessage::BuildAccepted { build_id } => {
            dispatch::handle_build_accepted(state, connection_id, build_id.0).await;
        }
        WorkerMessage::BuildStarted { build_id } => {
            dispatch::handle_build_started(state, build_id.0).await;
        }
        WorkerMessage::BuildOutput {
            build_id,
            start_seq,
            ref lines,
        } => {
            tracing::debug!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                build_id = %build_id,
                start_seq = start_seq,
                line_count = lines.len(),
                "build output"
            );
        }
        WorkerMessage::BuildFinished {
            build_id,
            status,
            ref error,
        } => {
            let status_str = match status {
                BuildFinishedStatus::Success => "success",
                BuildFinishedStatus::Failure => "failure",
                BuildFinishedStatus::Revoked => "revoked",
            };
            dispatch::handle_build_finished(
                state,
                connection_id,
                build_id.0,
                status_str,
                error.as_deref(),
            )
            .await;
        }
        WorkerMessage::BuildRejected {
            build_id,
            ref reason,
        } => {
            dispatch::handle_build_rejected(state, connection_id, build_id.0, reason).await;
        }
        WorkerMessage::WorkerStatus { state: ws, build_id } => {
            handle_worker_status(state, connection_id, worker_id, ws, build_id).await;
        }
        WorkerMessage::WorkerStopping {
            worker_id: ref wid,
            ref reason,
        } => {
            tracing::info!(
                connection_id = %connection_id,
                worker_id = %wid,
                reason = %reason,
                "worker stopping"
            );
            let mut queue = state.queue.lock().await;
            queue.set_worker_state(
                connection_id,
                WorkerState::Stopping {
                    worker_id: wid.clone(),
                },
            );
        }
    }
}

/// Reconnection decision table. Called when a worker sends `WorkerStatus`
/// after reconnecting. Implements the 10-row matrix from the design doc.
async fn handle_worker_status(
    state: &AppState,
    connection_id: &str,
    worker_id: &str,
    reported_state: WorkerReportedState,
    reported_build_id: Option<BuildId>,
) {
    tracing::info!(
        connection_id = %connection_id,
        worker_id = %worker_id,
        reported_state = ?reported_state,
        reported_build_id = ?reported_build_id,
        "processing worker status"
    );

    match (reported_state, reported_build_id) {
        // Worker reports it is building something.
        (WorkerReportedState::Building, Some(build_id)) => {
            let db_state = match crate::db::builds::get_build(&state.pool, build_id.0).await {
                Ok(Some(b)) => b.state,
                Ok(None) => "not_found".to_string(),
                Err(e) => {
                    tracing::error!(
                        build_id = build_id.0,
                        "reconnect: DB lookup failed: {e}"
                    );
                    return;
                }
            };

            match db_state.as_str() {
                "queued" => {
                    // Server thinks queued but worker is building — stale dispatch from
                    // a previous connection. Send revoke.
                    tracing::warn!(
                        build_id = build_id.0,
                        "reconnect: DB=queued but worker building — sending revoke"
                    );
                    let _ = dispatch::send_build_revoke(state, build_id.0).await;
                }
                "dispatched" => {
                    // Implicit accept — transition to started and resume.
                    tracing::info!(
                        build_id = build_id.0,
                        "reconnect: DB=dispatched, worker building — implicit accept → started"
                    );
                    dispatch::handle_build_started(state, build_id.0).await;

                    // Ensure active map has this build assigned to the new connection.
                    let mut queue = state.queue.lock().await;
                    if let Some(ab) = queue.active.get_mut(&build_id.0) {
                        ab.connection_id = connection_id.to_string();
                    }
                }
                "started" => {
                    // Resume — just reassign the connection in the active map.
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
                    // Re-send revoke.
                    tracing::info!(
                        build_id = build_id.0,
                        "reconnect: DB=revoking, worker building — re-sending revoke"
                    );
                    let _ = dispatch::send_build_revoke(state, build_id.0).await;
                }
                "failure" | "success" | "revoked" => {
                    // Terminal state but worker still building — send revoke.
                    tracing::warn!(
                        build_id = build_id.0,
                        db_state = %db_state,
                        "reconnect: terminal state but worker building — sending revoke"
                    );
                    let _ = dispatch::send_build_revoke(state, build_id.0).await;
                }
                "not_found" => {
                    // Build not in DB at all — send revoke to stop the worker.
                    tracing::warn!(
                        build_id = build_id.0,
                        "reconnect: build not found in DB — sending revoke"
                    );
                    // Can't use send_build_revoke (no active entry), send directly.
                    let msg = ServerMessage::BuildRevoke { build_id };
                    let json = serde_json::to_string(&msg)
                        .expect("ServerMessage serialization cannot fail");
                    let senders = state.worker_senders.lock().await;
                    if let Some(tx) = senders.get(connection_id) {
                        let _ = tx.send(Message::Text(json.into()));
                    }
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

        // Worker reports idle (no build, or explicitly idle).
        (WorkerReportedState::Idle, _) => {
            // Check if we have active builds assigned to any previous connection
            // for the same worker_id. We look for active builds where the
            // connection is this worker's previous (now-dead) connection.
            //
            // The queue's active map is keyed by build_id, so we scan for builds
            // that were dispatched to a connection that is now dead/disconnected.
            // However, since this is a *new* connection and the worker is idle,
            // we need to handle any builds the *old* connection had.
            //
            // For now, check if any build in the active map was assigned to a
            // disconnected/dead connection with this worker_id.
            let stale_builds: Vec<(i64, String)> = {
                let queue = state.queue.lock().await;
                queue
                    .active
                    .values()
                    .filter(|ab| {
                        // Check if the connection this build is assigned to is
                        // disconnected or dead (not the current connection).
                        ab.connection_id != connection_id
                            && queue
                                .get_worker(&ab.connection_id)
                                .map_or(true, |ws| {
                                    matches!(
                                        ws,
                                        WorkerState::Disconnected { .. } | WorkerState::Dead
                                    )
                                })
                    })
                    .map(|ab| (ab.build_id, ab.connection_id.clone()))
                    .collect()
            };

            for (build_id, old_cid) in &stale_builds {
                let db_state = match crate::db::builds::get_build(&state.pool, *build_id).await {
                    Ok(Some(b)) => b.state,
                    _ => continue,
                };

                match db_state.as_str() {
                    "dispatched" => {
                        // Worker never started the build — re-queue at front.
                        tracing::warn!(
                            build_id = build_id,
                            old_connection = %old_cid,
                            "reconnect idle: DB=dispatched — re-queuing"
                        );
                        requeue_active_build(state, *build_id).await;
                    }
                    "started" => {
                        // Worker lost the build while building.
                        tracing::error!(
                            build_id = build_id,
                            old_connection = %old_cid,
                            "reconnect idle: DB=started — marking FAILURE(worker lost build)"
                        );
                        fail_build(state, *build_id, "worker lost build").await;
                    }
                    _ => {}
                }
            }

            // Worker is idle — try dispatch.
            if let Err(dispatch::DispatchError::NothingToDispatch) =
                dispatch::try_dispatch(state).await
            {
                tracing::debug!(
                    connection_id = %connection_id,
                    "reconnect idle: no pending builds"
                );
            }
        }

        // Worker reports Building but with no build_id — treat as protocol error.
        (WorkerReportedState::Building, None) => {
            tracing::warn!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                "worker reports building but no build_id — ignoring"
            );
        }
    }
}

/// Handle a worker transitioning to Dead (grace period expired). Resolves
/// all active builds assigned to that connection.
pub async fn handle_worker_dead(state: &AppState, connection_id: &str) {
    let active_build_ids = {
        let queue = state.queue.lock().await;
        queue.active_builds_for_connection(connection_id)
    };

    for build_id in active_build_ids {
        let db_state = match crate::db::builds::get_build(&state.pool, build_id).await {
            Ok(Some(b)) => b.state,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!(build_id = build_id, "dead worker DB lookup failed: {e}");
                continue;
            }
        };

        match db_state.as_str() {
            "dispatched" | "started" => {
                tracing::error!(
                    build_id = build_id,
                    connection_id = %connection_id,
                    db_state = %db_state,
                    "worker dead — marking FAILURE(worker lost)"
                );
                fail_build(state, build_id, "worker lost").await;
            }
            "revoking" => {
                tracing::warn!(
                    build_id = build_id,
                    connection_id = %connection_id,
                    "worker dead while revoking — marking REVOKED"
                );
                if let Err(e) = crate::db::builds::set_build_finished(
                    &state.pool,
                    build_id,
                    "revoked",
                    Some("worker dead during revoke"),
                )
                .await
                {
                    tracing::error!(build_id = build_id, "failed to mark revoked: {e}");
                }
                if let Err(e) =
                    crate::db::builds::set_build_log_finished(&state.pool, build_id).await
                {
                    tracing::error!(build_id = build_id, "failed to mark log finished: {e}");
                }
                {
                    let mut queue = state.queue.lock().await;
                    queue.active.remove(&build_id);
                }
                {
                    let mut watchers = state.log_watchers.lock().await;
                    watchers.remove(&build_id);
                }
            }
            _ => {
                // Terminal state — just clean up active map.
                let mut queue = state.queue.lock().await;
                queue.active.remove(&build_id);
            }
        }
    }
}

/// Mark a build as FAILURE, clean up active map and log watchers.
async fn fail_build(state: &AppState, build_id: i64, reason: &str) {
    if let Err(e) =
        crate::db::builds::set_build_finished(&state.pool, build_id, "failure", Some(reason)).await
    {
        tracing::error!(build_id = build_id, "failed to mark build failed: {e}");
    }
    if let Err(e) = crate::db::builds::set_build_log_finished(&state.pool, build_id).await {
        tracing::error!(build_id = build_id, "failed to mark log finished: {e}");
    }
    {
        let mut queue = state.queue.lock().await;
        queue.active.remove(&build_id);
    }
    {
        let mut watchers = state.log_watchers.lock().await;
        watchers.remove(&build_id);
    }
}

/// Re-queue an active build at the front of its priority lane.
async fn requeue_active_build(state: &AppState, build_id: i64) {
    let active_build = {
        let mut queue = state.queue.lock().await;
        queue.active.remove(&build_id)
    };

    if let Some(ab) = active_build {
        if let Err(e) =
            crate::db::builds::update_build_state(&state.pool, build_id, "queued", None).await
        {
            tracing::error!(build_id = build_id, "failed to revert to queued: {e}");
        }

        let mut queue = state.queue.lock().await;
        queue.enqueue_front(crate::queue::QueuedBuild {
            build_id: BuildId(build_id),
            priority: cbsd_proto::Priority::Normal,
            descriptor: ab.descriptor,
            user_email: String::new(),
            queued_at: 0,
        });
    }

    // Remove watch sender (will be re-created on next dispatch).
    {
        let mut watchers = state.log_watchers.lock().await;
        watchers.remove(&build_id);
    }

    // Try to dispatch to another worker.
    if let Err(dispatch::DispatchError::NothingToDispatch) = dispatch::try_dispatch(state).await {
        tracing::debug!("no workers available after re-queue of build {build_id}");
    }
}

/// Clean up worker state on connection close.
async fn cleanup_worker(
    state: &AppState,
    connection_id: &str,
    worker_id: &str,
    is_stopping: bool,
) {
    // Remove the outbound sender channel.
    {
        let mut senders = state.worker_senders.lock().await;
        senders.remove(connection_id);
    }

    if is_stopping {
        // Worker announced graceful shutdown — skip grace period, go straight
        // to Dead. Re-queue any DISPATCHED builds (worker never started them).
        tracing::info!(
            connection_id = %connection_id,
            worker_id = %worker_id,
            "worker stopped — marking dead (immediate)"
        );

        {
            let mut queue = state.queue.lock().await;
            queue.set_worker_state(connection_id, WorkerState::Dead);
        }

        // Handle active builds: re-queue dispatched, fail started.
        handle_worker_dead(state, connection_id).await;
    } else {
        // Worker dropped unexpectedly — enter grace period.
        let arch = {
            let queue = state.queue.lock().await;
            queue.get_worker(connection_id).and_then(|w| w.arch())
        };

        if let Some(arch) = arch {
            tracing::warn!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                "worker disconnected — entering grace period"
            );

            {
                let mut queue = state.queue.lock().await;
                queue.set_worker_state(
                    connection_id,
                    WorkerState::Disconnected {
                        since: tokio::time::Instant::now(),
                        worker_id: worker_id.to_string(),
                        arch,
                    },
                );
            }

            // Start grace period monitor.
            let grace_secs = state.config.timeouts.liveness_grace_period_secs;
            crate::ws::liveness::start_grace_period_monitor(
                state,
                connection_id,
                grace_secs,
            );
        } else {
            tracing::warn!(
                connection_id = %connection_id,
                worker_id = %worker_id,
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
    }
}
