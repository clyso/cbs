// crt — thin git subprocess helpers.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Run `git` in a target repository directory (design §2/§4: subprocess git,
//! as `cbsd-worker` does). These run `git` with `current_dir(repo)` rather
//! than `git -C`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Run `git <args>` in `repo`; return stdout as a UTF-8 string.
pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let bytes = git_bytes(repo, args)?;
    String::from_utf8(bytes).with_context(|| format!("git {args:?}: output not UTF-8"))
}

/// Run `git <args>` in `repo`; return stdout as raw bytes.
pub fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .with_context(|| format!("spawning git {args:?}"))?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Feed `input` to `git <args>` in `repo` on stdin; return stdout (UTF-8).
///
/// Writes all of `input` before reading stdout, so use only for commands with
/// bounded output (e.g. `patch-id`); a command that emits large output while
/// still consuming stdin could deadlock.
pub fn git_with_stdin(repo: &Path, args: &[&str], input: &[u8]) -> Result<String> {
    let mut child = Command::new("git")
        .current_dir(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning git {args:?}"))?;
    {
        let mut stdin = child.stdin.take().context("git stdin unavailable")?;
        stdin.write_all(input).context("writing to git stdin")?;
    } // stdin closed here so git can finish
    let out = child
        .wait_with_output()
        .with_context(|| format!("git {args:?}"))?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).with_context(|| format!("git {args:?}: output not UTF-8"))
}
