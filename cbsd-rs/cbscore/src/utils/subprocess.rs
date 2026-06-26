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
//! non-zero exit). An optional async per-line `out_cb` streams each line as it
//! arrives **and** still collects it. This deliberately diverges from both
//! Python (whose `read_stream` returns empty strings when a callback is set) and
//! design 003: the port collects anyway so a non-zero exit keeps its stderr for
//! the error message — Python loses it, a latent bug for streamed commands. Its
//! first consumer is the in-container builder's toolchain install (M2/C2b).

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use camino::Utf8Path;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::Command;
use tracing::debug;

use crate::types::tracing_targets;
use crate::utils::redact::{CmdArg, sanitize_cmdline};

/// The future an [`OutCb`] returns for a single output line.
pub type OutLine = Pin<Box<dyn Future<Output = ()> + Send>>;

/// An async per-line output callback (design 003). Each line is awaited through
/// it as the process emits it (the trailing newline stripped). Its first
/// consumer is the in-container builder streaming its toolchain output.
pub type OutCb<'a> = dyn Fn(String) -> OutLine + Send + Sync + 'a;

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
    /// A per-line output callback. When set, each line is streamed through it as
    /// it arrives **and** still collected into the result. (Python and design 003
    /// leave the strings empty when a callback is set; the port collects anyway
    /// so a non-zero exit keeps its stderr for the error message.)
    pub out_cb: Option<&'a OutCb<'a>>,
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
    // pipe buffer cannot deadlock against our wait. With an `out_cb` each line
    // is streamed through it as it arrives and still collected (Python parity);
    // without one the streams are collected wholesale.
    let collect = async {
        let (read_out, read_err, status) = match opts.out_cb {
            None => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                let (ro, re, status) = tokio::join!(
                    child_out.read_to_string(&mut stdout),
                    child_err.read_to_string(&mut stderr),
                    child.wait(),
                );
                (ro.map(|_| stdout), re.map(|_| stderr), status)
            }
            Some(cb) => {
                tokio::join!(
                    stream_and_collect(&mut child_out, cb),
                    stream_and_collect(&mut child_err, cb),
                    child.wait(),
                )
            }
        };
        let stdout = read_out.map_err(CommandError::Io)?;
        let stderr = read_err.map_err(CommandError::Io)?;
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

/// Read a child stream line by line, awaiting `cb` for each line and collecting
/// them (newline-terminated) into the returned string. `lines()` strips the
/// terminator and normalises `\r\n`→`\n`, so the collected string is not
/// byte-exact with the raw output — fine for the log/error use here; the
/// no-callback path stays byte-exact via `read_to_string`.
async fn stream_and_collect<R: AsyncRead + Unpin>(
    reader: &mut R,
    cb: &OutCb<'_>,
) -> std::io::Result<String> {
    let mut lines = BufReader::new(reader).lines();
    let mut collected = String::new();
    while let Some(line) = lines.next_line().await? {
        cb(line.clone()).await;
        collected.push_str(&line);
        collected.push('\n');
    }
    Ok(collected)
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
    async fn out_cb_streams_each_line_and_still_collects() {
        use std::sync::{Arc, Mutex};

        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        let cb = move |line: String| -> OutLine {
            let sink = sink.clone();
            Box::pin(async move { sink.lock().unwrap().push(line) })
        };
        let args = [CmdArg::from("printf"), CmdArg::from("one\ntwo\n")];
        let out = run_cmd(
            &args,
            RunOpts {
                out_cb: Some(&cb),
                ..RunOpts::default()
            },
        )
        .await
        .unwrap();

        // Each line was streamed live...
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["one".to_string(), "two".to_string()]
        );
        // ...and the output is still collected (so errors keep their stderr).
        assert!(out.stdout.contains("one") && out.stdout.contains("two"));
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
