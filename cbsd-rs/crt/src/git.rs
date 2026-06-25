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

/// A patch to apply when materializing a release branch: its apply `order`, the
/// `blob_hash` recorded in the `Crt-Patch` trailer, and the raw
/// `git format-patch` mailbox bytes fed to `git am`.
pub struct PatchToApply {
    pub order: u32,
    pub blob_hash: String,
    pub bytes: Vec<u8>,
}

/// Build the linear `release/<name>` branch in `repo` (design §8): create an
/// isolated working copy at `base_ref`, then `git am` each patch in `order`,
/// amending a `Crt-Patch: sha256:<blob_hash>` trailer onto each resulting
/// commit. There is **no** `Crt-Visibility` trailer — visibility is store-only,
/// so the branch leaks no classification.
///
/// Fail-loud (design §8): any apply failure aborts the in-progress `am`, tears
/// down the temporary working copy, deletes the partial `branch`, and returns
/// the error — a half-materialized ref is never left behind. On success the
/// temporary working copy is removed and `branch` is left in `repo` at the last
/// applied commit. Returns the commit hashes in apply order; the caller uses
/// them only to report/inspect (the patch BOM is rebuilt from the trailers, not
/// from carried-forward hashes).
///
/// Subprocess `git`, blocking — a caller under an async runtime must offload it
/// (e.g. `tokio::task::spawn_blocking`). Content filters are disabled
/// (`core.autocrlf=false`) so applied blobs stay byte-faithful, and commits are
/// never GPG-signed — release authenticity comes from the detached `record.json`
/// signature and the annotated tag (§8); signing here would block on a
/// passphrase prompt. (The full content-filter audit is finalized later, §14.)
pub fn materialize_branch(
    repo: &Path,
    branch: &str,
    base_ref: &str,
    patches: &[PatchToApply],
) -> Result<Vec<String>> {
    // `git worktree add` requires its target path not to pre-exist, so create a
    // unique parent dir and join a child the parent does not create. The
    // `TempDir` is the cleanup backstop should a panic skip the explicit
    // `git worktree remove` below.
    let parent = tempfile::Builder::new()
        .prefix("crt-materialize-")
        .tempdir()
        .context("creating the materialization scratch directory")?;
    let work = parent.path().join("tree");
    let work_str = path_str(&work)?;

    git(
        repo,
        &[
            "-c",
            "core.autocrlf=false",
            "worktree",
            "add",
            "-b",
            branch,
            work_str,
            base_ref,
        ],
    )
    .with_context(|| format!("creating an isolated working copy for {branch} at {base_ref}"))?;

    let result = apply_patches(&work, patches);

    // Always tear down the temporary working copy; `branch` persists in `repo`.
    let _ = git(repo, &["worktree", "remove", "--force", work_str]);

    match result {
        Ok(commits) => Ok(commits),
        Err(e) => {
            // Fail loud: leave no half-materialized ref behind.
            let _ = git(repo, &["branch", "-D", branch]);
            Err(e)
        }
    }
}

/// `git am` each patch in `order` onto the working copy `work`, amending a
/// `Crt-Patch` trailer onto each commit. On any apply failure the in-progress
/// `am` is aborted before the error propagates, so the caller's teardown sees a
/// clean working copy. Returns the commit hashes in apply order.
fn apply_patches(work: &Path, patches: &[PatchToApply]) -> Result<Vec<String>> {
    let mut commits = Vec::with_capacity(patches.len());
    for p in patches {
        if let Err(e) = apply_one(work, p) {
            let _ = git(work, &["am", "--abort"]);
            return Err(e);
        }
        commits.push(git(work, &["rev-parse", "HEAD"])?.trim().to_owned());
    }
    Ok(commits)
}

/// `git am` one patch, then amend a `Crt-Patch: sha256:<blob_hash>` trailer onto
/// the resulting commit. The patch is staged to a scratch mailbox outside the
/// working copy so it never pollutes the tree.
fn apply_one(work: &Path, p: &PatchToApply) -> Result<()> {
    let mut mailbox =
        tempfile::NamedTempFile::new().context("creating a scratch mailbox for `git am`")?;
    mailbox
        .write_all(&p.bytes)
        .with_context(|| format!("writing patch order {} to the scratch mailbox", p.order))?;
    let mailbox_path = path_str(mailbox.path())?;

    git(
        work,
        &[
            "-c",
            "core.autocrlf=false",
            "-c",
            "commit.gpgsign=false",
            "am",
            mailbox_path,
        ],
    )
    .with_context(|| {
        format!(
            "`git am` failed for patch order {} (blob {})",
            p.order, p.blob_hash
        )
    })?;

    // Append the trailer via `interpret-trailers` so it lands in the commit's
    // trailer block regardless of the incoming message's shape.
    let message = git(work, &["log", "-1", "--format=%B"])?;
    let trailer = format!("Crt-Patch: sha256:{}", p.blob_hash);
    let amended = git_with_stdin(
        work,
        &["interpret-trailers", "--trailer", &trailer],
        message.as_bytes(),
    )?;
    git_with_stdin(
        work,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--amend",
            "--file=-",
        ],
        amended.as_bytes(),
    )
    .with_context(|| format!("amending the Crt-Patch trailer for order {}", p.order))?;
    Ok(())
}

/// A path as `&str` for a `git` argument; errors on non-UTF-8 (these are
/// program-constructed scratch paths, so this is effectively infallible).
fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .with_context(|| format!("path {} is not valid UTF-8", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Run git in `repo`, panicking on failure (test convenience).
    fn run(repo: &Path, args: &[&str]) -> String {
        git(repo, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    /// A fresh repo with a configured identity and one base commit on `main`.
    fn base_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-q", "-b", "main"]);
        run(p, &["config", "user.name", "Test Releaser"]);
        run(p, &["config", "user.email", "rel@example.com"]);
        std::fs::write(p.join("README.md"), "base\n").unwrap();
        run(p, &["add", "README.md"]);
        run(
            p,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "base: initial commit",
            ],
        );
        dir
    }

    /// Commit `content` to `file` on `main`, capture the patch via
    /// `format-patch`, then roll `main` back so the change lives only in the
    /// returned mailbox bytes (as an imported blob would).
    fn make_patch(repo: &Path, file: &str, content: &str, subject: &str) -> Vec<u8> {
        std::fs::write(repo.join(file), content).unwrap();
        run(repo, &["add", file]);
        run(
            repo,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", subject],
        );
        let bytes = git_bytes(repo, &["format-patch", "-1", "--stdout"]).unwrap();
        run(repo, &["reset", "--hard", "HEAD~1"]);
        bytes
    }

    /// The (single) `Crt-Patch` trailer value of `rev`.
    fn crt_patch_trailer(repo: &Path, rev: &str) -> String {
        run(
            repo,
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Crt-Patch,valueonly)",
                rev,
            ],
        )
        .trim()
        .to_owned()
    }

    #[test]
    fn materializes_a_linear_branch_with_crt_patch_trailers() {
        let dir = base_repo();
        let repo = dir.path();
        let p1 = make_patch(repo, "a.txt", "alpha\n", "feat: add a.txt");
        let p2 = make_patch(repo, "b.txt", "beta\n", "feat: add b.txt");
        let h1 = "a".repeat(64);
        let h2 = "b".repeat(64);
        let patches = vec![
            PatchToApply {
                order: 1,
                blob_hash: h1.clone(),
                bytes: p1,
            },
            PatchToApply {
                order: 2,
                blob_hash: h2.clone(),
                bytes: p2,
            },
        ];

        let commits = materialize_branch(repo, "release/ces-v1", "main", &patches).unwrap();
        assert_eq!(commits.len(), 2);

        // Linear: exactly the two applied commits sit on top of base.
        assert_eq!(
            run(repo, &["rev-list", "--count", "main..release/ces-v1"]).trim(),
            "2"
        );
        // Each commit carries its blob hash in a Crt-Patch trailer, in order.
        assert_eq!(crt_patch_trailer(repo, &commits[0]), format!("sha256:{h1}"));
        assert_eq!(crt_patch_trailer(repo, &commits[1]), format!("sha256:{h2}"));
        // The subjects survive the trailer amend; no Crt-Visibility ever leaks.
        assert!(run(repo, &["log", "-1", "--format=%s", &commits[0]]).contains("add a.txt"));
        for sha in &commits {
            assert!(
                !run(repo, &["log", "-1", "--format=%B", sha]).contains("Crt-Visibility"),
                "the branch must leak no visibility classification"
            );
        }
        // The scratch working copy was torn down; only the main one remains.
        assert_eq!(
            run(repo, &["worktree", "list"]).lines().count(),
            1,
            "the scratch working copy was not removed"
        );
    }

    #[test]
    fn an_apply_failure_aborts_and_leaves_no_branch() {
        let dir = base_repo();
        let repo = dir.path();
        // A patch that *modifies* a file absent at base_ref: build it as the
        // second commit atop a create, keep only the modify mailbox, then roll
        // base back past both — so a straight `git apply` has no parent blob to
        // patch and fails (no 3-way fallback is requested).
        std::fs::write(repo.join("data.txt"), "v1\n").unwrap();
        run(repo, &["add", "data.txt"]);
        run(
            repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "add data.txt",
            ],
        );
        std::fs::write(repo.join("data.txt"), "v2\n").unwrap();
        run(repo, &["add", "data.txt"]);
        run(
            repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "modify data.txt",
            ],
        );
        let bad = git_bytes(repo, &["format-patch", "-1", "--stdout"]).unwrap();
        run(repo, &["reset", "--hard", "HEAD~2"]);

        let patches = vec![PatchToApply {
            order: 1,
            blob_hash: "c".repeat(64),
            bytes: bad,
        }];
        let err = materialize_branch(repo, "release/bad", "main", &patches).unwrap_err();
        assert!(
            format!("{err:#}").contains("git am"),
            "expected a loud `git am` failure, got: {err:#}"
        );
        // No dangling ref and no leftover scratch working copy.
        assert!(
            git(repo, &["rev-parse", "--verify", "release/bad"]).is_err(),
            "a partial branch was left behind"
        );
        assert_eq!(
            run(repo, &["worktree", "list"]).lines().count(),
            1,
            "the scratch working copy was not cleaned up"
        );
    }
}
