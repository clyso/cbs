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

//! Output streaming from the build subprocess with batching.
//!
//! Reads stdout line-by-line, batching lines for efficiency (up to 50 lines
//! or 200ms, whichever comes first). Detects the structured `{"type":"result"}`
//! line emitted by the wrapper to extract the final exit status.

use std::time::Duration;

use cbsd_proto::build::BuildId;
use cbsd_proto::ws::{BuildFinishedStatus, WorkerMessage};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdout;
use tokio::sync::mpsc;

/// Maximum lines per batch before flushing.
const BATCH_MAX_LINES: usize = 50;

/// Maximum time to accumulate lines before flushing.
const BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(200);

/// Parsed result from the wrapper's structured output line.
#[derive(Debug)]
struct WrapperResult {
    exit_code: i32,
    error: Option<String>,
}

/// Errors during output streaming.
#[derive(Debug)]
pub enum OutputError {
    /// Failed to read from subprocess stdout.
    Read(std::io::Error),
    /// Failed to send a message to the WebSocket sender.
    Send(mpsc::error::SendError<WorkerMessage>),
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(err) => write!(f, "failed to read subprocess output: {err}"),
            Self::Send(err) => write!(f, "failed to send output message: {err}"),
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(err) => Some(err),
            Self::Send(err) => Some(err),
        }
    }
}

/// Stream output from the build subprocess stdout, batching lines for
/// efficiency.
///
/// Returns the final `(status, error)` extracted from the wrapper's structured
/// result line if present, or `(Failure, None)` if no result line was found.
pub async fn stream_output(
    stdout: ChildStdout,
    build_id: BuildId,
    sender: &mpsc::Sender<WorkerMessage>,
) -> Result<(BuildFinishedStatus, Option<String>), OutputError> {
    let reader = BufReader::new(stdout);
    let mut lines_iter = reader.lines();

    let mut line_count: u64 = 0;
    let mut batch: Vec<String> = Vec::with_capacity(BATCH_MAX_LINES);
    let mut batch_start_seq: u64 = 0;
    let mut wrapper_result: Option<WrapperResult> = None;

    let flush_timer = tokio::time::sleep(BATCH_FLUSH_INTERVAL);
    tokio::pin!(flush_timer);

    loop {
        tokio::select! {
            result = lines_iter.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        // Check for the structured result line.
                        if line.starts_with(r#"{"type":"result""#)
                            || line.starts_with(r#"{"type": "result""#)
                        {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
                                wrapper_result = Some(WrapperResult {
                                    exit_code: parsed
                                        .get("exit_code")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(-1) as i32,
                                    error: parsed
                                        .get("error")
                                        .and_then(|v| v.as_str())
                                        .map(String::from),
                                });
                            }
                            // Don't include the result line in output.
                            continue;
                        }

                        if batch.is_empty() {
                            batch_start_seq = line_count;
                            // Reset the flush timer when starting a new batch.
                            flush_timer.as_mut().reset(
                                tokio::time::Instant::now() + BATCH_FLUSH_INTERVAL,
                            );
                        }
                        batch.push(line);
                        line_count += 1;

                        // Flush if batch is full.
                        if batch.len() >= BATCH_MAX_LINES {
                            flush_batch(build_id, &mut batch, batch_start_seq, sender).await?;
                        }
                    }
                    Ok(None) => {
                        // EOF — flush remaining lines and exit.
                        if !batch.is_empty() {
                            flush_batch(build_id, &mut batch, batch_start_seq, sender).await?;
                        }
                        break;
                    }
                    Err(err) => {
                        // Flush what we have, then return error.
                        if !batch.is_empty() {
                            let _ = flush_batch(
                                build_id, &mut batch, batch_start_seq, sender,
                            ).await;
                        }
                        return Err(OutputError::Read(err));
                    }
                }
            }
            () = &mut flush_timer, if !batch.is_empty() => {
                flush_batch(build_id, &mut batch, batch_start_seq, sender).await?;
                // Reset timer (will be properly set when next batch starts).
                flush_timer.as_mut().reset(
                    tokio::time::Instant::now() + BATCH_FLUSH_INTERVAL,
                );
            }
        }
    }

    // Determine final status from the wrapper result.
    match wrapper_result {
        Some(wr) => {
            let status = super::executor::classify_exit_code(Some(wr.exit_code));
            Ok((status, wr.error))
        }
        None => {
            // No structured result line — treat as failure.
            Ok((BuildFinishedStatus::Failure, None))
        }
    }
}

/// Flush the current batch as a `BuildOutput` message.
async fn flush_batch(
    build_id: BuildId,
    batch: &mut Vec<String>,
    start_seq: u64,
    sender: &mpsc::Sender<WorkerMessage>,
) -> Result<(), OutputError> {
    let msg = WorkerMessage::BuildOutput {
        build_id,
        start_seq,
        lines: std::mem::take(batch),
    };
    sender.send(msg).await.map_err(OutputError::Send)
}
