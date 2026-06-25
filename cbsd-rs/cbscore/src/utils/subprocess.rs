// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! The async subprocess primitive (design 003), the single way every higher
//! subsystem reaches an external tool. Mirrors Python's `async_run_cmd`: it
//! inherits the environment (no PATH scrubbing — `cbsbuild` runs from no venv),
//! reads stdout and stderr concurrently, and wraps spawn+wait in a timeout.
//!
//! A non-zero exit is **not** an error of this primitive — it returns
//! [`CmdOutput`] with the code, and each wrapper decides whether that is fatal.
//! A spawn failure and a timeout/cancellation are distinct [`CommandError`]
//! variants (the latter resolves the Python FIXME that conflated timeout with a
//! non-zero exit). The async per-line `out_cb` lands with its first consumer
//! (the runner's live streaming, M2); C1 collects the output.

use std::process::Stdio;
use std::time::Duration;

use camino::Utf8Path;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::debug;

use crate::types::tracing_targets;
use crate::utils::redact::{CmdArg, sanitize_cmdline};

/// Options for [`run_cmd`].
#[derive(Default)]
pub struct RunOpts<'a> {
    /// Working directory for the child (default: inherit).
    pub cwd: Option<&'a Utf8Path>,
    /// Wall-clock deadline; on elapse the child is killed and [`CommandError::Timeout`]
    /// is returned.
    pub timeout: Option<Duration>,
    /// Extra environment, merged **over** the inherited environment.
    pub extra_env: &'a [(String, String)],
}

/// The captured result of a finished process. A non-zero `code` is a normal
/// outcome here, not an error.
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A failure of the subprocess primitive itself (distinct from a non-zero exit).
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("no command provided")]
    Empty,
    #[error("failed to spawn command")]
    Spawn(#[source] std::io::Error),
    #[error("error reading command output")]
    Io(#[source] std::io::Error),
    #[error("command timed out or was cancelled")]
    Timeout,
}

/// Run a command to completion, capturing stdout/stderr. The program and
/// arguments are the **plaintext** rendering of the `CmdArg`s; every log line
/// uses the redacted rendering.
pub async fn run_cmd(args: &[CmdArg], opts: RunOpts<'_>) -> Result<CmdOutput, CommandError> {
    let Some(program) = args.first() else {
        return Err(CommandError::Empty);
    };
    debug!(target: tracing_targets::SUBPROCESS, "run {:?}", sanitize_cmdline(args));

    let mut cmd = Command::new(program.plaintext());
    for arg in &args[1..] {
        cmd.arg(arg.plaintext());
    }
    if let Some(cwd) = opts.cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in opts.extra_env {
        cmd.env(key, value);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(CommandError::Spawn)?;
    let mut child_out = child.stdout.take().expect("stdout was piped");
    let mut child_err = child.stderr.take().expect("stderr was piped");

    // Read both streams concurrently with the wait, so a process that fills a
    // pipe buffer cannot deadlock against our wait.
    let collect = async {
        let mut stdout = String::new();
        let mut stderr = String::new();
        let (read_out, read_err, status) = tokio::join!(
            child_out.read_to_string(&mut stdout),
            child_err.read_to_string(&mut stderr),
            child.wait(),
        );
        read_out.map_err(CommandError::Io)?;
        read_err.map_err(CommandError::Io)?;
        let status = status.map_err(CommandError::Io)?;
        Ok::<CmdOutput, CommandError>(CmdOutput {
            code: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    };

    match opts.timeout {
        None => collect.await,
        Some(deadline) => {
            // Binding to `timed` ends the timeout future (and thus `collect`'s
            // borrow of `child`) before the kill path reuses `child`.
            let timed = tokio::time::timeout(deadline, collect).await;
            match timed {
                Ok(result) => result,
                Err(_elapsed) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    Err(CommandError::Timeout)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn captures_stdout_and_zero_exit() {
        let args = [CmdArg::from("printf"), CmdArg::from("hello")];
        let out = run_cmd(&args, RunOpts::default()).await.unwrap();
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, "hello");
    }

    #[tokio::test]
    async fn non_zero_exit_is_returned_not_errored() {
        let args = [
            CmdArg::from("sh"),
            CmdArg::from("-c"),
            CmdArg::from("exit 3"),
        ];
        let out = run_cmd(&args, RunOpts::default()).await.unwrap();
        assert_eq!(out.code, 3);
    }

    #[tokio::test]
    async fn spawn_failure_is_an_error() {
        let args = [CmdArg::from("definitely-not-a-real-binary-xyz")];
        assert!(matches!(
            run_cmd(&args, RunOpts::default()).await,
            Err(CommandError::Spawn(_))
        ));
    }

    #[tokio::test]
    async fn empty_args_is_an_error() {
        assert!(matches!(
            run_cmd(&[], RunOpts::default()).await,
            Err(CommandError::Empty)
        ));
    }

    #[tokio::test]
    async fn timeout_kills_and_reports_timeout() {
        let args = [CmdArg::from("sleep"), CmdArg::from("30")];
        let opts = RunOpts {
            timeout: Some(Duration::from_millis(100)),
            ..RunOpts::default()
        };
        assert!(matches!(
            run_cmd(&args, opts).await,
            Err(CommandError::Timeout)
        ));
    }

    #[tokio::test]
    async fn extra_env_is_merged_and_plaintext_reaches_exec() {
        // The spawned process sees the plaintext value even when carried as a
        // secret; the redacted form is what would be logged.
        let secret = CmdArg::Secure(std::sync::Arc::new(crate::utils::redact::Password::new(
            "$SECRET_VALUE",
        )));
        let args = [
            CmdArg::from("sh"),
            CmdArg::from("-c"),
            CmdArg::from("printf '%s' \"$SECRET_VALUE\""),
        ];
        let opts = RunOpts {
            extra_env: &[("SECRET_VALUE".to_string(), "plain-at-exec".to_string())],
            ..RunOpts::default()
        };
        let out = run_cmd(&args, opts).await.unwrap();
        assert_eq!(out.stdout, "plain-at-exec");
        // And the secret arg, if logged, is censored.
        assert_eq!(secret.redacted(), "<CENSORED>");
    }
}
