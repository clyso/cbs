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

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

use cbsd_proto::build::BuildId;
use cbsd_proto::ws::{BuildFinishedStatus, ServerMessage, WorkerMessage, WorkerReportedState};

use crate::build::{component, executor, output};
use crate::config::ResolvedWorkerConfig;
use crate::signal::ShutdownState;
use crate::ws::connection::WsStream;

/// Current protocol version.
const PROTOCOL_VERSION: u32 = 2;

/// Channel capacity for the output message sender.
const OUTPUT_CHANNEL_CAPACITY: usize = 64;

/// State tracking for an active build.
struct ActiveBuild {
    build_id: BuildId,
    executor: executor::BuildExecutor,
    component_dir: PathBuf,
}

/// Run a single WebSocket connection: send Hello, wait for Welcome, then
/// enter the message loop.
///
/// Returns `Err` when the connection is lost (triggers reconnect in the
/// caller). Returns `Ok(())` only on graceful shutdown.
pub async fn run_connection(
    stream: WsStream,
    config: &ResolvedWorkerConfig,
    state: Arc<ShutdownState>,
) -> Result<(), HandlerError> {
    let (mut sender, mut receiver) = stream.split();

    // --- Send Hello ---
    let hello = WorkerMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        arch: config.arch,
        cores_total: 0,  // TODO: populate from sysinfo
        ram_total_mb: 0, // TODO: populate from sysinfo
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
    // TODO: Check if there's an active build in executor state
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

    // Active build state — only one build at a time.
    let mut active_build: Option<ActiveBuild> = None;

    // Channel for build output messages from the background streaming task.
    let (output_tx, mut output_rx) = mpsc::channel::<WorkerMessage>(OUTPUT_CHANNEL_CAPACITY);

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
                    ServerMessage::BuildNew {
                        build_id,
                        trace_id,
                        priority,
                        descriptor,
                        component_sha256,
                    } => {
                        tracing::info!(
                            %build_id,
                            %trace_id,
                            ?priority,
                            "build dispatch received"
                        );

                        // If already building, reject.
                        if active_build.is_some() {
                            tracing::warn!(%build_id, "rejecting build: already building");
                            send_msg(
                                &mut sender,
                                &WorkerMessage::BuildRejected {
                                    build_id,
                                    reason: "worker is busy".to_string(),
                                },
                            ).await?;
                            continue;
                        }

                        // Read the next binary frame (component tarball).
                        let tarball = match read_binary_frame(&mut receiver).await {
                            Ok(data) => data,
                            Err(err) => {
                                tracing::error!(
                                    %build_id,
                                    %err,
                                    "failed to read component tarball"
                                );
                                send_msg(
                                    &mut sender,
                                    &WorkerMessage::BuildRejected {
                                        build_id,
                                        reason: format!(
                                            "failed to read component tarball: {err}"
                                        ),
                                    },
                                ).await?;
                                continue;
                            }
                        };

                        // Validate + unpack component.
                        let temp_dir = config
                            .component_temp_dir
                            .clone()
                            .unwrap_or_else(|| std::env::temp_dir().join("cbsd-components"));

                        if let Err(err) = std::fs::create_dir_all(&temp_dir) {
                            tracing::error!(
                                %build_id,
                                path = %temp_dir.display(),
                                %err,
                                "failed to create component temp dir"
                            );
                            send_msg(
                                &mut sender,
                                &WorkerMessage::BuildRejected {
                                    build_id,
                                    reason: format!(
                                        "failed to create temp directory: {err}"
                                    ),
                                },
                            ).await?;
                            continue;
                        }

                        let component_dir = match component::validate_and_unpack(
                            &tarball,
                            &component_sha256,
                            &temp_dir,
                        ) {
                            Ok(dir) => dir,
                            Err(err) => {
                                tracing::error!(
                                    %build_id,
                                    %err,
                                    "component validation failed"
                                );
                                send_msg(
                                    &mut sender,
                                    &WorkerMessage::BuildRejected {
                                        build_id,
                                        reason: "component integrity check failed".to_string(),
                                    },
                                ).await?;
                                continue;
                            }
                        };

                        // Accept the build.
                        send_msg(
                            &mut sender,
                            &WorkerMessage::BuildAccepted { build_id },
                        ).await?;
                        tracing::info!(%build_id, "build accepted");

                        // Spawn the build executor.
                        let mut exec = match executor::spawn_build(
                            config,
                            build_id,
                            &descriptor,
                            &component_dir,
                            &trace_id,
                        ).await {
                            Ok(e) => e,
                            Err(err) => {
                                tracing::error!(%build_id, %err, "failed to spawn build");
                                component::cleanup(&component_dir);
                                send_msg(
                                    &mut sender,
                                    &WorkerMessage::BuildFinished {
                                        build_id,
                                        status: BuildFinishedStatus::Failure,
                                        error: Some(format!("spawn failed: {err}")),
                                        build_report: None,
                                    },
                                ).await?;
                                continue;
                            }
                        };

                        // Send BuildStarted.
                        send_msg(
                            &mut sender,
                            &WorkerMessage::BuildStarted { build_id },
                        ).await?;
                        tracing::info!(%build_id, "build started");

                        // Take stdout for the output streamer.
                        let stdout = exec.child_mut().stdout.take();

                        // Spawn background output streaming task.
                        let bg_tx = output_tx.clone();
                        tokio::spawn(async move {
                            if let Some(stdout) = stdout {
                                match output::stream_output(stdout, build_id, &bg_tx).await {
                                    Ok((status, error, build_report)) => {
                                        if let Some(ref err_msg) = error {
                                            tracing::warn!(
                                                %build_id,
                                                error = %err_msg,
                                                ?status,
                                                "build wrapper reported error"
                                            );
                                        } else if status == BuildFinishedStatus::Success {
                                            let has_report = build_report.is_some();
                                            tracing::info!(
                                                %build_id,
                                                has_report,
                                                "build completed successfully"
                                            );
                                        }
                                        let _ = bg_tx.send(WorkerMessage::BuildFinished {
                                            build_id,
                                            status,
                                            error,
                                            build_report,
                                        }).await;
                                    }
                                    Err(err) => {
                                        tracing::error!(
                                            %build_id,
                                            %err,
                                            "output streaming failed"
                                        );
                                        let _ = bg_tx.send(WorkerMessage::BuildFinished {
                                            build_id,
                                            status: BuildFinishedStatus::Failure,
                                            error: Some(format!("output streaming error: {err}")),
                                            build_report: None,
                                        }).await;
                                    }
                                }
                            } else {
                                tracing::error!(
                                    %build_id,
                                    "no stdout on build subprocess"
                                );
                                let _ = bg_tx.send(WorkerMessage::BuildFinished {
                                    build_id,
                                    status: BuildFinishedStatus::Failure,
                                    error: Some("no stdout on subprocess".to_string()),
                                    build_report: None,
                                }).await;
                            }
                        });

                        active_build = Some(ActiveBuild {
                            build_id,
                            executor: exec,
                            component_dir,
                        });
                    }

                    ServerMessage::BuildRevoke { build_id } => {
                        tracing::info!(%build_id, "build revoke received");

                        if let Some(ref ab) = active_build {
                            if ab.build_id == build_id {
                                tracing::info!(
                                    %build_id,
                                    "killing active build process"
                                );
                                ab.executor.kill();
                                // The output streamer will detect the exit and
                                // send BuildFinished(revoked) via the channel.
                            } else {
                                tracing::warn!(
                                    %build_id,
                                    active = %ab.build_id,
                                    "revoke for non-active build, ignoring"
                                );
                            }
                        } else {
                            // No active build — pre-accept revoke.
                            tracing::info!(
                                %build_id,
                                "no active build, sending immediate BuildFinished(revoked)"
                            );
                            send_msg(
                                &mut sender,
                                &WorkerMessage::BuildFinished {
                                    build_id,
                                    status: BuildFinishedStatus::Revoked,
                                    error: None,
                                    build_report: None,
                                },
                            ).await?;
                        }
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

            // Forward output messages from the background streaming task.
            Some(output_msg) = output_rx.recv() => {
                let is_finished = matches!(output_msg, WorkerMessage::BuildFinished { .. });

                send_msg(&mut sender, &output_msg).await?;

                if is_finished {
                    // Clean up the active build.
                    if let Some(mut ab) = active_build.take() {
                        // Wait for the subprocess to fully exit.
                        let exit_code = ab.executor.wait().await;
                        let status = executor::classify_exit_code(exit_code);
                        tracing::info!(
                            build_id = %ab.build_id,
                            ?exit_code,
                            ?status,
                            "build subprocess exited"
                        );
                        component::cleanup(&ab.component_dir);
                    }
                }
            }

            () = state.notify.notified(), if !state.is_stopping() => {
                // Shutdown requested while in message loop.
                tracing::info!("shutdown requested, sending WorkerStopping");
                let stopping = WorkerMessage::WorkerStopping {
                    reason: "SIGTERM received".to_string(),
                };
                if let Ok(json) = serde_json::to_string(&stopping) {
                    let _ = sender.send(tungstenite::Message::Text(json)).await;
                }
                // Kill any active build before returning.
                if let Some(ref ab) = active_build {
                    ab.executor.kill();
                }
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Send a `WorkerMessage` as a JSON text frame.
async fn send_msg<S>(sender: &mut S, msg: &WorkerMessage) -> Result<(), HandlerError>
where
    S: SinkExt<tungstenite::Message, Error = tungstenite::Error> + Unpin,
{
    let json = serde_json::to_string(msg).map_err(HandlerError::Serialize)?;
    sender
        .send(tungstenite::Message::Text(json))
        .await
        .map_err(HandlerError::Send)
}

/// Read the next binary frame from the WebSocket stream, skipping pings/pongs.
///
/// Returns the binary data, or an error if a text/close frame is received
/// instead.
async fn read_binary_frame<S>(receiver: &mut S) -> Result<Vec<u8>, HandlerError>
where
    S: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    loop {
        let msg = receiver
            .next()
            .await
            .ok_or(HandlerError::ConnectionClosed)?
            .map_err(HandlerError::Receive)?;

        match msg {
            tungstenite::Message::Binary(data) => return Ok(data),
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => continue,
            tungstenite::Message::Close(_) => return Err(HandlerError::ConnectionClosed),
            other => {
                tracing::warn!(
                    ?other,
                    "expected binary frame for component tarball, got non-binary"
                );
                return Err(HandlerError::UnexpectedFrame);
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
    UnexpectedFrame,
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
            Self::UnexpectedFrame => write!(f, "unexpected frame type"),
        }
    }
}

impl std::error::Error for HandlerError {}
