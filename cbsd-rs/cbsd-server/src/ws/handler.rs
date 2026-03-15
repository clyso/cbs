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

use cbsd_proto::ws::{BuildFinishedStatus, ServerMessage, WorkerMessage};

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
            tracing::info!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                state = ?ws,
                build_id = ?build_id,
                "worker status (reconnection handling deferred to Commit 8b)"
            );
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

    let mut queue = state.queue.lock().await;
    if is_stopping {
        tracing::info!(
            connection_id = %connection_id,
            worker_id = %worker_id,
            "worker stopped — marking dead"
        );
        queue.set_worker_state(connection_id, WorkerState::Dead);
    } else {
        // Worker dropped unexpectedly — enter grace period.
        let arch = queue
            .get_worker(connection_id)
            .and_then(|w| w.arch());
        if let Some(arch) = arch {
            tracing::warn!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                "worker disconnected — entering grace period"
            );
            queue.set_worker_state(
                connection_id,
                WorkerState::Disconnected {
                    since: tokio::time::Instant::now(),
                    worker_id: worker_id.to_string(),
                    arch,
                },
            );
        } else {
            tracing::warn!(
                connection_id = %connection_id,
                worker_id = %worker_id,
                "worker disconnected with unknown arch — marking dead"
            );
            queue.set_worker_state(connection_id, WorkerState::Dead);
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
