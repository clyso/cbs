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

//! Build log file writer and sequence-to-offset index.
//!
//! Each active build has a log file under `{log_dir}/builds/{build_id}.log`.
//! The writer appends output lines, maintains an in-memory index mapping
//! `(line_seq) -> file_offset`, and notifies SSE followers via watch channels.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::app::LogWatchers;

/// In-memory state for the log writer.
///
/// Maps `build_id` to a vector of `(line_seq, file_offset)` pairs,
/// ordered by seq. This allows binary search to find the file offset
/// for any given sequence number (used by SSE follow to resume).
pub struct LogWriterState {
    pub(crate) seq_indices: HashMap<i64, Vec<(u64, u64)>>,
}

/// Thread-safe shared log writer.
pub type SharedLogWriter = Arc<Mutex<LogWriterState>>;

impl LogWriterState {
    pub fn new() -> Self {
        Self {
            seq_indices: HashMap::new(),
        }
    }
}

/// Append build output lines to the log file and update the seq-to-offset
/// index. Notifies any SSE watchers that new data is available.
///
/// - `log_dir`: base log directory (e.g., `./logs`)
/// - `build_id`: the build whose log file to append to
/// - `start_seq`: sequence number of the first line in `lines`
/// - `lines`: output lines (without trailing newlines)
pub async fn write_build_output(
    log_writer: &SharedLogWriter,
    log_watchers: &LogWatchers,
    log_dir: &Path,
    pool: &SqlitePool,
    build_id: i64,
    start_seq: u64,
    lines: &[String],
) -> Result<(), io::Error> {
    use tokio::io::AsyncWriteExt;

    if lines.is_empty() {
        return Ok(());
    }

    // Ensure log directory exists.
    let builds_dir = log_dir.join("builds");
    tokio::fs::create_dir_all(&builds_dir).await?;

    // Open log file in append mode (creates if missing).
    let log_path = builds_dir.join(format!("{build_id}.log"));
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await?;

    // Get the current file size (= offset for first write).
    let metadata = file.metadata().await?;
    let mut offset = metadata.len();

    // Build index entries and write lines.
    let mut new_entries = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let seq = start_seq + i as u64;
        new_entries.push((seq, offset));

        // Write line with newline terminator.
        let buf = format!("{line}\n");
        file.write_all(buf.as_bytes()).await?;
        offset += buf.len() as u64;
    }
    file.flush().await?;

    // Update in-memory seq-to-offset index.
    {
        let mut writer = log_writer.lock().await;
        let entries = writer.seq_indices.entry(build_id).or_default();
        entries.extend(new_entries);
    }

    // Notify SSE watchers (ignore if no watcher or receiver dropped).
    {
        let watchers = log_watchers.lock().await;
        if let Some(tx) = watchers.get(&build_id) {
            let _ = tx.send(());
        }
    }

    // Update log_size in DB (v1: every write; future: batched).
    let log_size = offset as i64;
    if let Err(e) = update_build_log_size(pool, build_id, log_size).await {
        tracing::warn!(build_id = build_id, "failed to update build log size: {e}");
    }

    Ok(())
}

/// Finalize a build's log: drop the in-memory index and mark the log
/// as finished in the database.
pub async fn finish_build_log(log_writer: &SharedLogWriter, pool: &SqlitePool, build_id: i64) {
    // Drop the seq-to-offset index for this build.
    {
        let mut writer = log_writer.lock().await;
        writer.seq_indices.remove(&build_id);
    }

    // Mark the log row as finished in the DB.
    if let Err(e) = crate::db::builds::set_build_log_finished(pool, build_id).await {
        tracing::error!(
            build_id = build_id,
            "failed to mark build log finished: {e}"
        );
    }
}

/// Look up the file offset for a given sequence number using binary search.
///
/// Returns `None` if the build is not in the index or the sequence number
/// has not been written yet.
pub async fn get_seq_offset(log_writer: &SharedLogWriter, build_id: i64, seq: u64) -> Option<u64> {
    let writer = log_writer.lock().await;
    let entries = writer.seq_indices.get(&build_id)?;

    // Binary search for the seq number.
    match entries.binary_search_by_key(&seq, |(s, _)| *s) {
        Ok(idx) => Some(entries[idx].1),
        Err(_) => None,
    }
}

/// Update `build_logs.log_size` in the database.
async fn update_build_log_size(
    pool: &SqlitePool,
    build_id: i64,
    log_size: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE build_logs SET log_size = ?, updated_at = unixepoch() WHERE build_id = ?")
        .bind(log_size)
        .bind(build_id)
        .execute(pool)
        .await?;

    Ok(())
}
