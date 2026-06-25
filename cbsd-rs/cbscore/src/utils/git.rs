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

//! The `git` shell-tool wrapper (design 003). C1 lands `run_git` and the two
//! read-only ops `versions create` needs (`get_git_user`, `get_git_repo_root`);
//! the clone/checkout/worktree/apply operations land with component preparation
//! (C3, design 007).

use camino::{Utf8Path, Utf8PathBuf};

use crate::utils::redact::CmdArg;
use crate::utils::subprocess::{CommandError, RunOpts, run_cmd};

/// An error from a git invocation.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// git exited non-zero; `msg` carries its stderr.
    #[error("git error: {msg} (retcode: {retcode})")]
    NonZero { retcode: i32, msg: String },
    /// A required `git config` value is empty/unset.
    #[error("{what} not set in config")]
    ConfigNotSet { what: String },
    /// The git process could not be spawned, read, or timed out.
    #[error("git command failed to run")]
    Command(#[from] CommandError),
}

/// Run a git command. Prepends `git`, and `-C <path>` when `path` is given.
/// Returns stdout on success, [`GitError::NonZero`] on a non-zero exit.
pub async fn run_git(args: &[CmdArg], path: Option<&Utf8Path>) -> Result<String, GitError> {
    let mut cmd: Vec<CmdArg> = Vec::with_capacity(args.len() + 3);
    cmd.push(CmdArg::from("git"));
    if let Some(p) = path {
        cmd.push(CmdArg::from("-C"));
        cmd.push(CmdArg::from(p.as_str()));
    }
    cmd.extend_from_slice(args);

    let out = run_cmd(&cmd, RunOpts::default()).await?;
    if out.code != 0 {
        return Err(GitError::NonZero {
            retcode: out.code,
            msg: out.stderr,
        });
    }
    Ok(out.stdout)
}

/// The current repository's git user name and email (`git config user.name` /
/// `user.email`). [`GitError::ConfigNotSet`] when either is empty.
pub async fn get_git_user() -> Result<(String, String), GitError> {
    async fn config_value(key: &str) -> Result<String, GitError> {
        let value = run_git(&[CmdArg::from("config"), CmdArg::from(key)], None).await?;
        let value = value.trim();
        if value.is_empty() {
            return Err(GitError::ConfigNotSet {
                what: key.to_string(),
            });
        }
        Ok(value.to_string())
    }

    let name = config_value("user.name").await?;
    let email = config_value("user.email").await?;
    Ok((name, email))
}

/// The root of the current git repository (`git rev-parse --show-toplevel`).
pub async fn get_git_repo_root() -> Result<Utf8PathBuf, GitError> {
    let value = run_git(
        &[CmdArg::from("rev-parse"), CmdArg::from("--show-toplevel")],
        None,
    )
    .await?;
    let value = value.trim();
    if value.is_empty() {
        return Err(GitError::NonZero {
            retcode: 0,
            msg: "top-level git directory not found".to_string(),
        });
    }
    Ok(Utf8PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    /// Initialise a throwaway git repo with a configured user in `dir`.
    async fn init_repo(dir: &Utf8Path) {
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Test User"],
            vec!["config", "user.email", "test@example.com"],
        ] {
            let status = Command::new("git")
                .args(&args)
                .current_dir(dir)
                .status()
                .await
                .expect("git available");
            assert!(status.success());
        }
    }

    #[tokio::test]
    async fn user_and_repo_root_from_a_real_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        init_repo(dir).await;

        // Run from inside the repo (the C1 ops take no path). NOTE: this mutates
        // the process-global cwd, which parallel tests share — keep this the
        // *only* cwd-mutating test in the crate (the end-to-end tests in
        // cbsbuild/tests exercise these ops via a subprocess instead). cwd is
        // restored below before any assertion can unwind.
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();

        let user = get_git_user().await;
        let root = get_git_repo_root().await;

        std::env::set_current_dir(prev).unwrap();

        let (name, email) = user.unwrap();
        assert_eq!(name, "Test User");
        assert_eq!(email, "test@example.com");
        // The toplevel resolves to the temp dir (modulo symlinks like /var → /private).
        assert!(root.unwrap().as_str().ends_with(dir.file_name().unwrap()));
    }
}
