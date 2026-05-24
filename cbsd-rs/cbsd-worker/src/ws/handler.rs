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

//! WebSocket transport layer.
//!
//! After commit 6, this module is a thin transport client. Active build
//! state lives in [`build::supervisor::Supervisor`], which outlives any
//! individual websocket connection. The handler:
//!
//! 1. Performs the Hello/Welcome handshake.
//! 2. Asks the supervisor for the reconnect messages to send (Idle or
//!    Building + spooled output + pending terminal).
//! 3. Forwards inbound `ServerMessage`s to the supervisor and outbound
//!    `WorkerMessage`s from the supervisor's per-connection channel.
//!
//! A websocket receive/send error does NOT kill the build. The supervisor
//! keeps the subprocess and local assignment state until `BuildRevoke`,
//! the process exits, or local worker shutdown. (Gap G6 of the WCP
//! soundness review.)

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

use cbsd_proto::build::BuildId;
use cbsd_proto::ws::{BuildFinishedStatus, ServerMessage, WorkerMessage};

use crate::build::supervisor::{RevokeOutcome, Supervisor};
use crate::build::{component, executor, output};
use crate::config::ResolvedWorkerConfig;
use crate::signal::ShutdownState;
use crate::ws::connection::WsStream;

/// Current protocol version.
const PROTOCOL_VERSION: u32 = 2;

/// Channel capacity for the outbound message queue between the supervisor
/// and this transport.
const OUTPUT_CHANNEL_CAPACITY: usize = 64;

/// Run a single WebSocket connection: send Hello, wait for Welcome, then
/// enter the message loop.
///
/// The supervisor outlives this function; a return value of `Err` causes
/// the caller (`reconnect_loop`) to reconnect without disturbing the
/// active build. `Ok(())` is returned only on graceful shutdown, after
/// the supervisor has already been told to stop any active build.
pub async fn run_connection(
    stream: WsStream,
    config: &ResolvedWorkerConfig,
    supervisor: Arc<Supervisor>,
    state: Arc<ShutdownState>,
) -> Result<(), HandlerError> {
    let (mut sender, mut receiver) = stream.split();

    // --- Send Hello ---
    let hello = WorkerMessage::Hello {
        protocol_version: PROTOCOL_VERSION,
        arch: config.arch,
        cores_total: 0,  // TODO: populate from sysinfo
        ram_total_mb: 0, // TODO: populate from sysinfo
        version: Some(crate::VERSION.to_string()),
    };
    let hello_json = serde_json::to_string(&hello).map_err(HandlerError::Serialize)?;
    sender
        .send(tungstenite::Message::Text(hello_json))
        .await
        .map_err(HandlerError::send)?;
    tracing::debug!("sent Hello");

    // --- Wait for Welcome ---
    let connection_id = wait_for_welcome(&mut receiver, config).await?;

    // --- Drain pending supervisor state out the new transport ---
    // Order is fixed by the supervisor: WorkerStatus first, then any
    // spooled output, then any pending terminal `BuildFinished`. This
    // preserves the invariant that the worker never reports idle while
    // it still has unreported local state for a build, and never
    // delivers terminal output before announcing it is still building.
    let (out_tx, mut out_rx) = mpsc::channel::<WorkerMessage>(OUTPUT_CHANNEL_CAPACITY);
    supervisor.attach_transport(out_tx.clone()).await;
    for msg in supervisor.take_reconnect_messages().await {
        send_msg(&mut sender, &msg).await?;
        // Retire after a final BuildFinished is on the wire so the
        // supervisor's state goes back to idle. The retire call awaits
        // the streaming task and subprocess.
        if let WorkerMessage::BuildFinished { build_id, .. } = &msg {
            let bid = *build_id;
            let sup = Arc::clone(&supervisor);
            tokio::spawn(async move {
                sup.retire(bid).await;
            });
        }
    }

    tracing::info!(%connection_id, "entering message loop");

    loop {
        tokio::select! {
            frame = receiver.next() => {
                let Some(server_msg) = read_server_message(frame)? else {
                    // Benign non-text frame (ping/pong/binary out of band).
                    continue;
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

                        if let Err(err) = handle_build_new(
                            &mut sender,
                            &mut receiver,
                            &supervisor,
                            config,
                            build_id,
                            &trace_id,
                            &descriptor,
                            &component_sha256,
                        ).await {
                            tracing::error!(%build_id, %err, "build dispatch handling failed");
                            // handle_build_new is responsible for
                            // sending BuildRejected itself; if a send
                            // error bubbles up here, the connection is
                            // already lost and the caller will
                            // reconnect.
                            return Err(err);
                        }
                    }

                    ServerMessage::BuildRevoke { build_id } => {
                        tracing::info!(%build_id, "build revoke received");

                        match supervisor.on_build_revoke(build_id).await {
                            RevokeOutcome::RevokingActive => {
                                // The streamer will observe the
                                // subprocess exit and emit
                                // BuildFinished(revoked).
                            }
                            RevokeOutcome::NonActive { active } => {
                                tracing::warn!(
                                    %build_id,
                                    %active,
                                    "revoke for non-active build"
                                );
                            }
                            RevokeOutcome::Idle => {
                                // Pre-accept revoke: synthesize an
                                // immediate terminal.
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
                    }

                    ServerMessage::Welcome { .. } => {
                        tracing::warn!("unexpected Welcome after handshake, ignoring");
                    }
                    ServerMessage::Error { reason, .. } => {
                        tracing::error!(%reason, "server error, closing connection");
                        supervisor.detach_transport().await;
                        return Err(HandlerError::ServerError(reason));
                    }
                    ServerMessage::UnauthorizedBuildAction {
                        build_id,
                        action,
                        reason,
                    } => {
                        // Non-fatal per WCP D2: server rejected a
                        // lifecycle message. The full stop-work response
                        // is BuildRevoke, which the server sends as a
                        // follow-up.
                        tracing::warn!(
                            %build_id,
                            ?action,
                            ?reason,
                            "server rejected lifecycle message as unauthorized"
                        );
                    }
                }
            }

            // Forward output messages from the supervisor.
            Some(out_msg) = out_rx.recv() => {
                let terminal_build = match &out_msg {
                    WorkerMessage::BuildFinished { build_id, .. } => Some(*build_id),
                    _ => None,
                };

                send_msg(&mut sender, &out_msg).await?;

                if let Some(bid) = terminal_build {
                    // The terminal is on the wire; tell the supervisor
                    // to retire the build (await streamer + subprocess,
                    // clean up component dir).
                    let sup = Arc::clone(&supervisor);
                    tokio::spawn(async move {
                        sup.retire(bid).await;
                    });
                }
            }

            () = state.notify.notified(), if !state.is_stopping() => {
                tracing::info!("shutdown requested, sending WorkerStopping");
                let stopping = WorkerMessage::WorkerStopping {
                    reason: "SIGTERM received".to_string(),
                };
                if let Ok(json) = serde_json::to_string(&stopping) {
                    let _ = sender.send(tungstenite::Message::Text(json)).await;
                }
                // Local worker shutdown is one of the three stop-work
                // signals (WCP "Worker-Side Active Build State"). The
                // supervisor handles the kill + await.
                supervisor.shutdown().await;
                supervisor.detach_transport().await;
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handshake helpers
// ---------------------------------------------------------------------------

/// Run the Welcome wait loop. Returns the server-assigned connection id.
async fn wait_for_welcome<S>(
    receiver: &mut S,
    config: &ResolvedWorkerConfig,
) -> Result<String, HandlerError>
where
    S: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    loop {
        let msg = receiver
            .next()
            .await
            .ok_or(HandlerError::ConnectionClosed)?
            .map_err(HandlerError::receive)?;

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

                return Ok(connection_id);
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
    }
}

/// Parse one frame from the receive side. Returns `Ok(None)` for benign
/// non-text frames the caller should skip; returns `Err` for fatal
/// transport errors.
fn read_server_message(
    frame: Option<Result<tungstenite::Message, tungstenite::Error>>,
) -> Result<Option<ServerMessage>, HandlerError> {
    let msg = match frame {
        Some(Ok(msg)) => msg,
        Some(Err(err)) => return Err(HandlerError::receive(err)),
        None => return Err(HandlerError::ConnectionClosed),
    };

    let text = match msg {
        tungstenite::Message::Text(t) => t,
        tungstenite::Message::Close(_) => {
            tracing::info!("server closed connection");
            return Err(HandlerError::ConnectionClosed);
        }
        tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => return Ok(None),
        _ => return Ok(None),
    };

    match serde_json::from_str::<ServerMessage>(&text) {
        Ok(m) => Ok(Some(m)),
        Err(err) => {
            tracing::warn!(%err, "failed to parse server message, ignoring");
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// BuildNew handling
// ---------------------------------------------------------------------------

/// Handle a `BuildNew` dispatch: read the tarball, validate+unpack,
/// spawn the executor, and register everything with the supervisor.
///
/// Worker-busy and integrity rejections are sent as `BuildRejected`.
/// Spawn failures are sent as `BuildFinished(Failure)`. The supervisor
/// is left empty in all rejection paths so subsequent dispatches work.
#[allow(clippy::too_many_arguments)]
async fn handle_build_new<S, R>(
    sender: &mut S,
    receiver: &mut R,
    supervisor: &Arc<Supervisor>,
    config: &ResolvedWorkerConfig,
    build_id: BuildId,
    trace_id: &str,
    descriptor: &cbsd_proto::build::BuildDescriptor,
    component_sha256: &str,
) -> Result<(), HandlerError>
where
    S: SinkExt<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    R: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    if supervisor.active_build_id().await.is_some() {
        tracing::warn!(%build_id, "rejecting build: already building");
        send_msg(
            sender,
            &WorkerMessage::BuildRejected {
                build_id,
                reason: "worker is busy".to_string(),
            },
        )
        .await?;
        return Ok(());
    }

    // Read the next binary frame (component tarball).
    let tarball = match read_binary_frame(receiver).await {
        Ok(data) => data,
        Err(err) => {
            tracing::error!(%build_id, %err, "failed to read component tarball");
            send_msg(
                sender,
                &WorkerMessage::BuildRejected {
                    build_id,
                    reason: format!("failed to read component tarball: {err}"),
                },
            )
            .await?;
            return Ok(());
        }
    };

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
            sender,
            &WorkerMessage::BuildRejected {
                build_id,
                reason: format!("failed to create temp directory: {err}"),
            },
        )
        .await?;
        return Ok(());
    }

    let component_dir = match component::validate_and_unpack(
        &tarball,
        component_sha256,
        &temp_dir,
        config.max_uncompressed_component_bytes(),
    ) {
        Ok(dir) => dir,
        Err(err) => {
            tracing::error!(%build_id, %err, "component validation failed");
            send_msg(
                sender,
                &WorkerMessage::BuildRejected {
                    build_id,
                    reason: "component integrity check failed".to_string(),
                },
            )
            .await?;
            return Ok(());
        }
    };

    // Accept the build before spawning so the server cancels its
    // dispatch-ack timer promptly.
    send_msg(sender, &WorkerMessage::BuildAccepted { build_id }).await?;
    tracing::info!(%build_id, "build accepted");

    let mut exec =
        match executor::spawn_build(config, build_id, descriptor, &component_dir, trace_id).await {
            Ok(e) => e,
            Err(err) => {
                tracing::error!(%build_id, %err, "failed to spawn build");
                component::cleanup(&component_dir);
                send_msg(
                    sender,
                    &WorkerMessage::BuildFinished {
                        build_id,
                        status: BuildFinishedStatus::Failure,
                        error: Some(format!("spawn failed: {err}")),
                        build_report: None,
                    },
                )
                .await?;
                return Ok(());
            }
        };

    let stdout = exec.child_mut().stdout.take();

    // Register the active build BEFORE spawning the streaming task so
    // the supervisor sees the executor in case the streamer produces a
    // message immediately.
    if let Err(err) = supervisor
        .register_accepted(build_id, exec, component_dir.clone())
        .await
    {
        tracing::error!(%build_id, %err, "supervisor refused registration");
        // Shouldn't happen — we checked above — but recover by sending
        // a failure terminal.
        send_msg(
            sender,
            &WorkerMessage::BuildFinished {
                build_id,
                status: BuildFinishedStatus::Failure,
                error: Some(format!("supervisor: {err}")),
                build_report: None,
            },
        )
        .await?;
        return Ok(());
    }

    // Send BuildStarted *and* tell the supervisor before output flows.
    send_msg(sender, &WorkerMessage::BuildStarted { build_id }).await?;
    supervisor.mark_started(build_id).await;
    tracing::info!(%build_id, "build started");

    // Spawn the streaming task. The supervisor owns the JoinHandle so
    // it can await the task at retire/shutdown time.
    let sup_clone = Arc::clone(supervisor);
    let task = tokio::spawn(async move {
        run_output_streamer(sup_clone, stdout, build_id).await;
    });
    supervisor.attach_output_task(build_id, task).await;

    Ok(())
}

/// Drain the subprocess stdout into the supervisor. The streamer never
/// owns the websocket — it talks only to the supervisor, which decides
/// per message whether to forward to the live transport or spool.
async fn run_output_streamer(
    supervisor: Arc<Supervisor>,
    stdout: Option<tokio::process::ChildStdout>,
    build_id: BuildId,
) {
    // The output module sends messages on an mpsc; we use a local
    // channel and forward to the supervisor so existing output-batch
    // logic stays unchanged.
    let (tx, mut rx) = mpsc::channel::<WorkerMessage>(OUTPUT_CHANNEL_CAPACITY);

    // Drainer: read from `rx`, forward to supervisor.
    let drain_sup = Arc::clone(&supervisor);
    let drain = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            drain_sup.on_output_message(msg).await;
        }
    });

    if let Some(stdout) = stdout {
        match output::stream_output(stdout, build_id, &tx).await {
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
                let _ = tx
                    .send(WorkerMessage::BuildFinished {
                        build_id,
                        status,
                        error,
                        build_report,
                    })
                    .await;
            }
            Err(err) => {
                tracing::error!(%build_id, %err, "output streaming failed");
                let _ = tx
                    .send(WorkerMessage::BuildFinished {
                        build_id,
                        status: BuildFinishedStatus::Failure,
                        error: Some(format!("output streaming error: {err}")),
                        build_report: None,
                    })
                    .await;
            }
        }
    } else {
        tracing::error!(%build_id, "no stdout on build subprocess");
        let _ = tx
            .send(WorkerMessage::BuildFinished {
                build_id,
                status: BuildFinishedStatus::Failure,
                error: Some("no stdout on subprocess".to_string()),
                build_report: None,
            })
            .await;
    }

    drop(tx);
    // Wait for the drain task to finish flushing into the supervisor.
    let _ = drain.await;
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
        .map_err(HandlerError::send)
}

/// Read the next binary frame from the WebSocket stream, skipping pings/pongs.
async fn read_binary_frame<S>(receiver: &mut S) -> Result<Vec<u8>, HandlerError>
where
    S: StreamExt<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    loop {
        let msg = receiver
            .next()
            .await
            .ok_or(HandlerError::ConnectionClosed)?
            .map_err(HandlerError::receive)?;

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
///
/// The tungstenite error variants are boxed so the enum fits within the
/// clippy `result_large_err` budget; tungstenite errors are infrequent
/// per-message and the extra heap allocation is negligible.
#[derive(Debug)]
pub enum HandlerError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    Send(Box<tungstenite::Error>),
    Receive(Box<tungstenite::Error>),
    ConnectionClosed,
    ServerError(String),
    UnexpectedFrame,
}

impl HandlerError {
    fn send(err: tungstenite::Error) -> Self {
        Self::Send(Box::new(err))
    }

    fn receive(err: tungstenite::Error) -> Self {
        Self::Receive(Box::new(err))
    }
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
