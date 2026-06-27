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

//! The `git` shell-tool wrapper (design 003). C1 landed `run_git` and the two
//! read-only ops `versions create` needs (`get_git_user`, `get_git_repo_root`);
//! C3 adds the clone/checkout/worktree/apply/sha1 operations component
//! preparation drives (design 007). The clone `repo` argument is a [`CmdArg`] so
//! a credentialed URL is redacted in logs.

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
    /// A filesystem operation around a git command failed (mkdir, removing an
    /// invalid mirror).
    #[error("git io error ({context})")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
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

/// Clone a `--mirror` of `repo` into `<base_path>/<repo_name>.git`, or update it
/// in place if a valid mirror already exists there (`git_clone`, `git.py`). The
/// `repo` is a [`CmdArg`] so a credentialed URL stays redacted in logs. Returns
/// the mirror path.
pub async fn git_clone(
    repo: CmdArg,
    base_path: &Utf8Path,
    repo_name: &str,
) -> Result<Utf8PathBuf, GitError> {
    if !tokio::fs::try_exists(base_path).await.unwrap_or(false) {
        tokio::fs::create_dir_all(base_path)
            .await
            .map_err(|source| GitError::Io {
                context: format!("creating base path '{base_path}'"),
                source,
            })?;
    }

    let dest = base_path.join(format!("{repo_name}.git"));
    if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        let is_dir = tokio::fs::metadata(&dest)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let has_head = tokio::fs::try_exists(dest.join("HEAD"))
            .await
            .unwrap_or(false);
        if !is_dir || !has_head {
            tokio::fs::remove_dir_all(&dest)
                .await
                .map_err(|source| GitError::Io {
                    context: format!("removing invalid mirror '{dest}'"),
                    source,
                })?;
        }
        // Faithful to `git.py`: an existing destination is always *updated*, even
        // right after a nuke above — so a corrupt mirror surfaces as an update
        // error rather than being re-cloned. A latent Python quirk, reproduced;
        // flag for ROADMAP rather than "fix" mid-port.
        update_mirror(&repo, &dest).await?;
        return Ok(dest);
    }

    clone_mirror(&repo, &dest).await?;
    Ok(dest)
}

/// `git clone --mirror --quiet <repo> <dest>`.
async fn clone_mirror(repo: &CmdArg, dest: &Utf8Path) -> Result<(), GitError> {
    run_git(
        &[
            CmdArg::from("clone"),
            CmdArg::from("--mirror"),
            CmdArg::from("--quiet"),
            repo.clone(),
            CmdArg::from(dest.as_str()),
        ],
        None,
    )
    .await?;
    Ok(())
}

/// Point an existing mirror at `repo` and fetch (`remote set-url` + `remote
/// update`).
async fn update_mirror(repo: &CmdArg, repo_path: &Utf8Path) -> Result<(), GitError> {
    run_git(
        &[
            CmdArg::from("remote"),
            CmdArg::from("set-url"),
            CmdArg::from("origin"),
            repo.clone(),
        ],
        Some(repo_path),
    )
    .await?;
    run_git(
        &[CmdArg::from("remote"), CmdArg::from("update")],
        Some(repo_path),
    )
    .await?;
    Ok(())
}

/// Check `git_ref` out into a fresh worktree on a new branch under
/// `worktrees_base` (`git_checkout`, `git.py`). The branch/worktree name is the
/// ref with `/`→`--` plus a random hex suffix, so concurrent checkouts of the
/// same ref do not collide. Returns the worktree path.
///
/// `worktrees_base` must be **absolute**: the command runs under `-C repo_path`,
/// so a relative base would resolve against the repo, not the caller. Python
/// `.resolve()`d these; the port's caller (component prep) builds them under the
/// absolute scratch root, so it absolutizes once there rather than per-op.
pub async fn git_checkout(
    repo_path: &Utf8Path,
    git_ref: &str,
    worktrees_base: &Utf8Path,
) -> Result<Utf8PathBuf, GitError> {
    tokio::fs::create_dir_all(worktrees_base)
        .await
        .map_err(|source| GitError::Io {
            context: format!("creating worktrees base '{worktrees_base}'"),
            source,
        })?;

    let worktree_name = format!("{}.{}", git_ref.replace('/', "--"), random_hex_suffix());
    let worktree_path = worktrees_base.join(&worktree_name);
    run_git(
        &[
            CmdArg::from("worktree"),
            CmdArg::from("add"),
            CmdArg::from("--track"),
            CmdArg::from("-b"),
            CmdArg::from(worktree_name.as_str()),
            CmdArg::from("--quiet"),
            CmdArg::from(worktree_path.as_str()),
            CmdArg::from(git_ref),
        ],
        Some(repo_path),
    )
    .await?;
    Ok(worktree_path)
}

/// Remove a worktree (`git worktree remove --force`).
pub async fn git_remove_worktree(
    repo_path: &Utf8Path,
    worktree_path: &Utf8Path,
) -> Result<(), GitError> {
    run_git(
        &[
            CmdArg::from("worktree"),
            CmdArg::from("remove"),
            CmdArg::from("--force"),
            CmdArg::from(worktree_path.as_str()),
        ],
        Some(repo_path),
    )
    .await?;
    Ok(())
}

/// Apply a patch to the repository at `repo_path` (`git apply <patch>`).
/// `patch_path` must be **absolute** (the command runs under `-C repo_path`).
pub async fn git_apply(repo_path: &Utf8Path, patch_path: &Utf8Path) -> Result<(), GitError> {
    run_git(
        &[CmdArg::from("apply"), CmdArg::from(patch_path.as_str())],
        Some(repo_path),
    )
    .await?;
    Ok(())
}

/// The currently checked-out commit of the repository at `repo_path`
/// (`git rev-parse HEAD`).
pub async fn git_get_sha1(repo_path: &Utf8Path) -> Result<String, GitError> {
    let value = run_git(
        &[CmdArg::from("rev-parse"), CmdArg::from("HEAD")],
        Some(repo_path),
    )
    .await?;
    let value = value.trim();
    if value.is_empty() {
        return Err(GitError::NonZero {
            retcode: 0,
            msg: format!("no HEAD sha1 for repository '{repo_path}'"),
        });
    }
    Ok(value.to_string())
}

/// Ten random lowercase hex characters (five CSPRNG bytes), the collision-
/// avoiding worktree suffix (Python's `secrets.token_hex(5)`).
fn random_hex_suffix() -> String {
    use rand::Rng;
    let bytes: [u8; 5] = rand::rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

    async fn git_cmd(dir: &Utf8Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .await
            .expect("git available");
        assert!(status.success(), "git {args:?} failed");
    }

    /// A source repo with one commit and a `testref` branch; returns its HEAD
    /// sha. The C3 ops take explicit paths, so (unlike the C1 test above) these
    /// never mutate the process cwd.
    async fn init_source(dir: &Utf8Path) -> String {
        init_repo(dir).await;
        tokio::fs::write(dir.join("README"), "hello\n")
            .await
            .unwrap();
        git_cmd(dir, &["add", "."]).await;
        git_cmd(dir, &["commit", "-q", "-m", "init"]).await;
        git_cmd(dir, &["branch", "testref"]).await;
        run_git(
            &[CmdArg::from("rev-parse"), CmdArg::from("HEAD")],
            Some(dir),
        )
        .await
        .unwrap()
        .trim()
        .to_string()
    }

    const PATCH: &str = "--- a/README\n+++ b/README\n@@ -1 +1,2 @@\n hello\n+patched\n";

    #[tokio::test]
    async fn clone_checkout_sha1_apply_then_remove_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let src = base.join("source");
        tokio::fs::create_dir(&src).await.unwrap();
        let head = init_source(&src).await;

        // Mirror-clone the source, then check out `testref` into a worktree.
        let mirror = git_clone(CmdArg::from(src.as_str()), &base.join("repos"), "src")
            .await
            .unwrap();
        assert!(
            tokio::fs::try_exists(mirror.join("HEAD")).await.unwrap(),
            "the mirror has a HEAD"
        );

        let worktree = git_checkout(&mirror, "testref", &base.join("worktrees"))
            .await
            .unwrap();
        assert!(
            tokio::fs::try_exists(worktree.join("README"))
                .await
                .unwrap()
        );

        // The checked-out sha matches the source HEAD.
        assert_eq!(git_get_sha1(&worktree).await.unwrap(), head);

        // Apply a patch into the worktree.
        let patch = base.join("change.patch");
        tokio::fs::write(&patch, PATCH).await.unwrap();
        git_apply(&worktree, &patch).await.unwrap();
        let readme = tokio::fs::read_to_string(worktree.join("README"))
            .await
            .unwrap();
        assert!(readme.contains("patched"), "patch applied: {readme}");

        // Remove the worktree.
        git_remove_worktree(&mirror, &worktree).await.unwrap();
        assert!(!tokio::fs::try_exists(&worktree).await.unwrap());
    }

    #[tokio::test]
    async fn git_clone_updates_an_existing_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let src = base.join("source");
        tokio::fs::create_dir(&src).await.unwrap();
        init_source(&src).await;

        let repos = base.join("repos");
        let first = git_clone(CmdArg::from(src.as_str()), &repos, "src")
            .await
            .unwrap();
        // A second clone to the same name takes the update-in-place path.
        let second = git_clone(CmdArg::from(src.as_str()), &repos, "src")
            .await
            .unwrap();
        assert_eq!(first, second);
        assert!(tokio::fs::try_exists(second.join("HEAD")).await.unwrap());
    }
}
