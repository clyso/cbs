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

//! Build executor: spawns the cbscore wrapper subprocess and manages its
//! lifecycle (SIGTERM/SIGKILL escalation, process group isolation).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cbsd_proto::build::{BuildDescriptor, BuildId};
use cbsd_proto::ws::BuildFinishedStatus;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

use crate::config::ResolvedWorkerConfig;

/// Default SIGTERM → SIGKILL escalation timeout in seconds.
const DEFAULT_SIGKILL_TIMEOUT_SECS: u64 = 15;

/// Manages a running build subprocess.
pub struct BuildExecutor {
    /// The child process handle.
    child: Child,
    /// Build identifier for logging.
    build_id: BuildId,
    /// Process ID (cached from spawn for signal delivery after child is consumed).
    pid: u32,
    /// Set to `true` when `kill()` has been called.
    cancelled: Arc<AtomicBool>,
    /// Timeout before escalating SIGTERM to SIGKILL.
    sigkill_timeout: Duration,
}

/// Errors from build executor operations.
#[derive(Debug)]
pub enum ExecutorError {
    /// Failed to resolve the wrapper script path.
    WrapperNotFound(PathBuf),
    /// A required config field is missing.
    MissingConfig(&'static str),
    /// Failed to serialize the build input to JSON.
    SerializeInput(serde_json::Error),
    /// Failed to spawn the subprocess.
    Spawn(std::io::Error),
    /// Failed to write to the subprocess stdin.
    WriteStdin(std::io::Error),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrapperNotFound(path) => {
                write!(f, "cbscore wrapper not found: {}", path.display())
            }
            Self::MissingConfig(field) => {
                write!(f, "missing required config: {field}")
            }
            Self::SerializeInput(err) => write!(f, "failed to serialize build input: {err}"),
            Self::Spawn(err) => write!(f, "failed to spawn build subprocess: {err}"),
            Self::WriteStdin(err) => {
                write!(f, "failed to write to subprocess stdin: {err}")
            }
        }
    }
}

impl std::error::Error for ExecutorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WrapperNotFound(_) | Self::MissingConfig(_) => None,
            Self::SerializeInput(err) => Some(err),
            Self::Spawn(err) | Self::WriteStdin(err) => Some(err),
        }
    }
}

/// Resolve the wrapper script path from config or default.
fn resolve_wrapper_path(config: &ResolvedWorkerConfig) -> PathBuf {
    if let Some(ref path) = config.cbscore_wrapper_path {
        path.clone()
    } else {
        // Default: look relative to the current executable.
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("scripts").join("cbscore-wrapper.py")
    }
}

/// Spawn a build subprocess.
///
/// The subprocess runs `python3 <wrapper>` with the build descriptor written
/// to its stdin as JSON. The process is placed in its own process group via
/// `setsid()` for clean signal delivery.
pub async fn spawn_build(
    config: &ResolvedWorkerConfig,
    build_id: BuildId,
    descriptor: &BuildDescriptor,
    component_path: &Path,
    trace_id: &str,
) -> Result<BuildExecutor, ExecutorError> {
    let wrapper_path = resolve_wrapper_path(config);
    if !wrapper_path.exists() {
        return Err(ExecutorError::WrapperNotFound(wrapper_path));
    }

    // Guard: cbscore config is required for builds.
    let cbscore_config_path = config
        .cbscore_config_path
        .as_ref()
        .ok_or(ExecutorError::MissingConfig("cbscore-config-path"))?;

    // Build the JSON payload for stdin.
    let input = serde_json::json!({
        "descriptor": descriptor,
        "component_path": component_path.to_string_lossy(),
        "trace_id": trace_id,
    });
    let input_bytes = serde_json::to_vec(&input).map_err(ExecutorError::SerializeInput)?;

    let sigkill_timeout = Duration::from_secs(
        config
            .sigkill_escalation_timeout_secs
            .unwrap_or(DEFAULT_SIGKILL_TIMEOUT_SECS),
    );

    // Spawn the subprocess with process group isolation.
    let mut cmd = Command::new("python3");
    cmd.arg(&wrapper_path)
        .env("CBS_TRACE_ID", trace_id)
        .env("CBSCORE_CONFIG", cbscore_config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()); // wrapper redirects stderr→stdout via os.dup2

    // Optional env vars.
    if let Some(timeout) = config.build_timeout_secs {
        cmd.env("CBS_BUILD_TIMEOUT", timeout.to_string());
    }

    // SAFETY: `setsid()` is async-signal-safe (POSIX). This is the only
    // operation in the `pre_exec` hook.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn().map_err(ExecutorError::Spawn)?;

    // Write descriptor to stdin and close it.
    let pid = child.id().unwrap_or(0);
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&input_bytes)
            .await
            .map_err(ExecutorError::WriteStdin)?;
        // Drop stdin to signal EOF to the subprocess.
        drop(stdin);
    }

    tracing::info!(
        %build_id,
        pid,
        wrapper = %wrapper_path.display(),
        "spawned build subprocess"
    );

    Ok(BuildExecutor {
        child,
        build_id,
        pid,
        cancelled: Arc::new(AtomicBool::new(false)),
        sigkill_timeout,
    })
}

impl BuildExecutor {
    /// Access the child process (for taking stdout).
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// The build identifier.
    #[allow(dead_code)]
    pub fn build_id(&self) -> BuildId {
        self.build_id
    }

    /// Whether `kill()` has been called.
    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Send SIGTERM to the process group. Spawns a background task that
    /// escalates to SIGKILL after `sigkill_timeout` if the process is still
    /// running.
    pub fn kill(&self) {
        if self.cancelled.swap(true, Ordering::Relaxed) {
            // Already cancelled.
            return;
        }

        let pid = self.pid;
        let pgid = -(pid as i32);

        tracing::info!(
            build_id = %self.build_id,
            pid,
            "sending SIGTERM to process group"
        );

        // SAFETY: Sending SIGTERM to a process group is safe. The negative PID
        // targets the entire process group created by setsid() in pre_exec.
        unsafe {
            libc::kill(pgid, libc::SIGTERM);
        }

        // Spawn escalation task.
        let timeout = self.sigkill_timeout;
        let build_id = self.build_id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            tracing::warn!(
                %build_id,
                pid,
                "escalating to SIGKILL after {timeout:?} timeout"
            );
            // SAFETY: Same as above — sending SIGKILL to the process group.
            unsafe {
                libc::kill(pgid, libc::SIGKILL);
            }
        });
    }

    /// Wait for the child process to exit and return the exit code.
    pub async fn wait(&mut self) -> Option<i32> {
        match self.child.wait().await {
            Ok(status) => status.code(),
            Err(err) => {
                tracing::error!(
                    build_id = %self.build_id,
                    %err,
                    "failed to wait for build subprocess"
                );
                None
            }
        }
    }
}

/// Map a subprocess exit code to a `BuildFinishedStatus`.
///
/// - `0` → `Success`
/// - `137` (SIGKILL = 128+9) → `Revoked`
/// - `143` (SIGTERM = 128+15) → `Revoked`
/// - Any other code or `None` (signal without code) → `Failure`
pub fn classify_exit_code(code: Option<i32>) -> BuildFinishedStatus {
    match code {
        Some(0) => BuildFinishedStatus::Success,
        Some(137) | Some(143) => BuildFinishedStatus::Revoked,
        _ => BuildFinishedStatus::Failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_success() {
        assert_eq!(classify_exit_code(Some(0)), BuildFinishedStatus::Success);
    }

    #[test]
    fn classify_failure() {
        assert_eq!(classify_exit_code(Some(1)), BuildFinishedStatus::Failure);
        assert_eq!(classify_exit_code(Some(2)), BuildFinishedStatus::Failure);
        assert_eq!(classify_exit_code(Some(127)), BuildFinishedStatus::Failure);
    }

    #[test]
    fn classify_revoked_sigkill() {
        assert_eq!(classify_exit_code(Some(137)), BuildFinishedStatus::Revoked);
    }

    #[test]
    fn classify_revoked_sigterm() {
        assert_eq!(classify_exit_code(Some(143)), BuildFinishedStatus::Revoked);
    }

    #[test]
    fn classify_none_is_failure() {
        assert_eq!(classify_exit_code(None), BuildFinishedStatus::Failure);
    }
}
