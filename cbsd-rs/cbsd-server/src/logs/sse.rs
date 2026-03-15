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

//! SSE (Server-Sent Events) endpoint for real-time log following.
//!
//! The SSE stream emits `event: output` with `id: <seq>` for each log line,
//! then `event: done` when the build finishes. Clients can resume by passing
//! the `Last-Event-ID` header to seek into the log file.

use std::convert::Infallible;

use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use sqlx::SqlitePool;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};

use crate::app::{AppState, LogWatchers};
use crate::logs::writer::SharedLogWriter;

/// SSE follow handler. Called from the route handler with extracted parameters.
///
/// Takes `AppState` by value (it is cheaply clonable via Arc internals).
/// Opens the log file once and holds the FD for the stream lifetime (design
/// constraint: prevents GC race on Linux where an open FD survives unlink).
///
/// Returns an SSE stream that:
/// - Emits existing lines from the current file position
/// - Waits on the watch channel for new data notifications
/// - Emits `event: done` when the build log is marked finished
pub async fn sse_follow(
    state: AppState,
    build_id: i64,
    last_event_id: Option<u64>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let pool = &state.pool;

    // Check build exists.
    crate::db::builds::get_build(pool, build_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("database error: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, "build not found".to_string()))?;

    // Check build_logs row exists.
    let log_row = get_build_log_row(pool, build_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("database error: {e}"),
        )
    })?;
    let log_row = log_row.ok_or((StatusCode::NOT_FOUND, "no logs yet".to_string()))?;

    // Determine log file path.
    let log_file_path = state.config.log_dir.join(&log_row.log_path);

    // Try to open the log file.
    let file = match tokio::fs::File::open(&log_file_path).await {
        Ok(f) => Some(f),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to open log file: {e}"),
            ));
        }
    };

    // If last_event_id is provided, seek to the right position.
    let start_seq = last_event_id.map(|id| id + 1).unwrap_or(0);
    let seek_offset = if start_seq > 0 {
        crate::logs::writer::get_seq_offset(&state.log_writer, build_id, start_seq).await
    } else {
        None
    };

    // Subscribe to the watch channel (if exists) for new data notifications.
    let watch_rx = {
        let watchers = state.log_watchers.lock().await;
        watchers.get(&build_id).map(|tx| tx.subscribe())
    };

    let pool = pool.clone();
    let log_writer = state.log_writer.clone();
    let log_watchers = state.log_watchers.clone();
    let log_file_path_owned = log_file_path.clone();
    let finished_at_start = log_row.finished;
    let no_file = file.is_none();

    let stream = async_stream::stream! {
        // Current sequence counter (tracks what we've emitted).
        let mut current_seq = start_seq;

        // Handle the case: finished build with no log file.
        if finished_at_start && no_file {
            yield Ok::<_, Infallible>(
                Event::default().event("done").data("build complete"),
            );
            return;
        }

        // Open file (or wait for it).
        let mut reader = if let Some(mut f) = file {
            if let Some(offset) = seek_offset {
                if let Err(e) = f.seek(std::io::SeekFrom::Start(offset)).await {
                    tracing::warn!(build_id = build_id, "seek failed: {e}");
                }
            }
            Some(BufReader::new(f))
        } else {
            None
        };

        // If the build is already finished, read everything and emit done.
        if finished_at_start {
            for event in read_available_lines(&mut reader, build_id, &mut current_seq).await {
                yield Ok::<_, Infallible>(event);
            }
            yield Ok(Event::default().event("done").data("build complete"));
            return;
        }

        // Build is active — read available lines, then wait for notifications.
        let mut watch_rx = watch_rx;

        loop {
            // Try to open the file if we don't have it yet.
            if reader.is_none() {
                if let Ok(f) = tokio::fs::File::open(&log_file_path_owned).await {
                    reader = Some(BufReader::new(f));
                }
            }

            // Read all available lines from current position.
            for event in read_available_lines(&mut reader, build_id, &mut current_seq).await {
                yield Ok::<_, Infallible>(event);
            }

            // Check if log is now finished.
            let is_finished = match get_build_log_row(&pool, build_id).await {
                Ok(Some(row)) => row.finished,
                _ => false,
            };

            if is_finished {
                // Read any remaining lines after the finished flag was set.
                for event in read_available_lines(&mut reader, build_id, &mut current_seq).await {
                    yield Ok::<_, Infallible>(event);
                }
                yield Ok(Event::default().event("done").data("build complete"));
                return;
            }

            // Wait for notification of new data (or channel drop = build done).
            match watch_rx {
                Some(ref mut rx) => {
                    if rx.changed().await.is_err() {
                        // Sender dropped — build finished. Do a final read pass.
                        for event in read_available_lines(
                            &mut reader, build_id, &mut current_seq,
                        ).await {
                            yield Ok::<_, Infallible>(event);
                        }
                        yield Ok(Event::default().event("done").data("build complete"));
                        return;
                    }
                    // Wakeup received — loop back to read new lines.
                }
                None => {
                    // No watch channel — try to subscribe via LogWatchers.
                    let new_rx = try_subscribe(&log_watchers, &log_writer, build_id).await;
                    if new_rx.is_some() {
                        watch_rx = new_rx;
                    } else {
                        // Poll fallback — wait a bit and check again.
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Read all available lines from the reader and return them as SSE events.
async fn read_available_lines(
    reader: &mut Option<BufReader<tokio::fs::File>>,
    build_id: i64,
    current_seq: &mut u64,
) -> Vec<Event> {
    let mut events = Vec::new();
    if let Some(r) = reader.as_mut() {
        let mut line = String::new();
        loop {
            line.clear();
            match r.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim_end_matches('\n').to_string();
                    events.push(
                        Event::default()
                            .event("output")
                            .id(current_seq.to_string())
                            .data(trimmed),
                    );
                    *current_seq += 1;
                }
                Err(e) => {
                    tracing::warn!(build_id = build_id, "read error: {e}");
                    break;
                }
            }
        }
    }
    events
}

/// Try to subscribe to the watch channel for a build. Returns `Some` if the
/// watch sender exists (build is active), `None` otherwise.
async fn try_subscribe(
    log_watchers: &LogWatchers,
    log_writer: &SharedLogWriter,
    build_id: i64,
) -> Option<tokio::sync::watch::Receiver<()>> {
    // Check if the writer has entries for this build (indicates active).
    let has_entries = {
        let writer = log_writer.lock().await;
        writer.seq_indices.contains_key(&build_id)
    };

    if has_entries {
        let watchers = log_watchers.lock().await;
        watchers.get(&build_id).map(|tx| tx.subscribe())
    } else {
        None
    }
}

/// Extract the `Last-Event-ID` header from request headers.
pub fn parse_last_event_id(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Minimal build log row for SSE logic.
struct BuildLogRow {
    log_path: String,
    finished: bool,
}

/// Query the build_logs row for a given build.
async fn get_build_log_row(
    pool: &SqlitePool,
    build_id: i64,
) -> Result<Option<BuildLogRow>, sqlx::Error> {
    use sqlx::Row;

    let row = sqlx::query(
        "SELECT log_path, finished FROM build_logs WHERE build_id = ?",
    )
    .bind(build_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| {
        let finished_int: i32 = r.get("finished");
        BuildLogRow {
            log_path: r.get("log_path"),
            finished: finished_int != 0,
        }
    }))
}
