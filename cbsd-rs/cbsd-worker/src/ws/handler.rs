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

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite;

use cbsd_proto::ws::{ServerMessage, WorkerMessage, WorkerReportedState};

use crate::config::WorkerConfig;
use crate::signal::ShutdownState;
use crate::ws::connection::WsStream;

/// Current protocol version.
const PROTOCOL_VERSION: u32 = 1;

/// Run a single WebSocket connection: send Hello, wait for Welcome, then
/// enter the message loop.
///
/// Returns `Err` when the connection is lost (triggers reconnect in the
/// caller). Returns `Ok(())` only on graceful shutdown.
pub async fn run_connection(
    stream: WsStream,
    config: &WorkerConfig,
    state: Arc<ShutdownState>,
) -> Result<(), HandlerError> {
    let (mut sender, mut receiver) = stream.split();

    // --- Send Hello ---
    let hello = WorkerMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        worker_id: config.worker_id.clone(),
        arch: config.parsed_arch(),
        cores_total: 0,   // TODO: populate from sysinfo
        ram_total_mb: 0,   // TODO: populate from sysinfo
    };
    let hello_json = serde_json::to_string(&hello).map_err(HandlerError::Serialize)?;
    sender
        .send(tungstenite::Message::Text(hello_json))
        .await
        .map_err(HandlerError::Send)?;
    tracing::debug!("sent Hello");

    // --- Wait for Welcome ---
    let connection_id = loop {
        let msg = receiver
            .next()
            .await
            .ok_or(HandlerError::ConnectionClosed)?
            .map_err(HandlerError::Receive)?;

        let text = match msg {
            tungstenite::Message::Text(t) => t,
            tungstenite::Message::Close(_) => return Err(HandlerError::ConnectionClosed),
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => continue,
            other => {
                tracing::debug!(?other, "ignoring non-text frame while waiting for Welcome");
                continue;
            }
        };

        let server_msg: ServerMessage =
            serde_json::from_str(&text).map_err(HandlerError::Deserialize)?;

        match server_msg {
            ServerMessage::Welcome {
                protocol_version,
                connection_id,
                grace_period_secs,
            } => {
                tracing::info!(
                    %connection_id,
                    protocol_version,
                    grace_period_secs,
                    "received Welcome"
                );

                // Validate backoff ceiling against grace period.
                let ceiling = config.backoff_ceiling_secs();
                if ceiling >= grace_period_secs {
                    tracing::warn!(
                        ceiling,
                        grace_period_secs,
                        "backoff ceiling >= server grace period; \
                         clamping ceiling to {clamped}s",
                        clamped = grace_period_secs.saturating_sub(1)
                    );
                }

                break connection_id;
            }
            ServerMessage::Error {
                reason,
                min_version,
                max_version,
            } => {
                tracing::error!(
                    %reason,
                    ?min_version,
                    ?max_version,
                    "server rejected connection"
                );
                return Err(HandlerError::ServerError(reason));
            }
            other => {
                tracing::warn!(?other, "unexpected message before Welcome, ignoring");
            }
        }
    };

    // --- Report status on reconnect (if mid-build) ---
    // TODO(commit-11): Check if there's an active build in executor state
    // and send WorkerStatus { state: Building, build_id }.
    // For now, report idle.
    let status = WorkerMessage::WorkerStatus {
        state: WorkerReportedState::Idle,
        build_id: None,
    };
    let status_json = serde_json::to_string(&status).map_err(HandlerError::Serialize)?;
    sender
        .send(tungstenite::Message::Text(status_json))
        .await
        .map_err(HandlerError::Send)?;
    tracing::debug!("sent WorkerStatus (idle)");

    // --- Message loop ---
    tracing::info!(%connection_id, "entering message loop");

    loop {
        tokio::select! {
            frame = receiver.next() => {
                let msg = match frame {
                    Some(Ok(msg)) => msg,
                    Some(Err(err)) => return Err(HandlerError::Receive(err)),
                    None => return Err(HandlerError::ConnectionClosed),
                };

                let text = match msg {
                    tungstenite::Message::Text(t) => t,
                    tungstenite::Message::Close(_) => {
                        tracing::info!("server closed connection");
                        return Err(HandlerError::ConnectionClosed);
                    }
                    tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => continue,
                    _ => continue,
                };

                let server_msg: ServerMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(err) => {
                        tracing::warn!(%err, "failed to parse server message, ignoring");
                        continue;
                    }
                };

                match server_msg {
                    ServerMessage::BuildNew { build_id, trace_id, priority, .. } => {
                        // TODO(commit-11): Wire up executor to accept and run the build.
                        tracing::info!(
                            %build_id,
                            %trace_id,
                            ?priority,
                            "build dispatch received (executor not yet wired)"
                        );
                    }
                    ServerMessage::BuildRevoke { build_id } => {
                        // TODO(commit-11): Wire up executor to cancel the build.
                        tracing::info!(
                            %build_id,
                            "build revoke received (executor not yet wired)"
                        );
                    }
                    ServerMessage::Welcome { .. } => {
                        tracing::warn!("unexpected Welcome after handshake, ignoring");
                    }
                    ServerMessage::Error { reason, .. } => {
                        tracing::error!(%reason, "server error, closing connection");
                        return Err(HandlerError::ServerError(reason));
                    }
                }
            }
            () = state.notify.notified(), if !state.is_stopping() => {
                // Shutdown requested while in message loop.
                tracing::info!("shutdown requested, sending WorkerStopping");
                let stopping = WorkerMessage::WorkerStopping {
                    worker_id: config.worker_id.clone(),
                    reason: "SIGTERM received".to_string(),
                };
                if let Ok(json) = serde_json::to_string(&stopping) {
                    let _ = sender.send(tungstenite::Message::Text(json)).await;
                }
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors during a WebSocket session.
#[derive(Debug)]
pub enum HandlerError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    Send(tungstenite::Error),
    Receive(tungstenite::Error),
    ConnectionClosed,
    ServerError(String),
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(err) => write!(f, "serialize error: {err}"),
            Self::Deserialize(err) => write!(f, "deserialize error: {err}"),
            Self::Send(err) => write!(f, "send error: {err}"),
            Self::Receive(err) => write!(f, "receive error: {err}"),
            Self::ConnectionClosed => write!(f, "connection closed"),
            Self::ServerError(reason) => write!(f, "server error: {reason}"),
        }
    }
}

impl std::error::Error for HandlerError {}
