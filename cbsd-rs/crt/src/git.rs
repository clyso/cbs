// crt — thin git subprocess helpers.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Run `git` in a target repository directory (design §2/§4: subprocess git,
//! as `cbsd-worker` does). These run `git` with `current_dir(repo)` rather
//! than `git -C`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use tempfile::TempDir;

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

/// A scratch git worktree holding a freshly built release branch, kept alive so
/// the caller can append the signed `000-RELEASE/` commit and the annotated tag
/// before teardown (design §8; see [`crate::bundle`]). The branch persists in
/// `repo` after [`Worktree::remove`]; the `TempDir` is only a directory-cleanup
/// backstop if an explicit teardown is skipped (e.g. on panic) — call `remove`
/// or `cleanup_failed` so `git worktree` also forgets the registration.
#[derive(Debug)]
pub struct Worktree {
    repo: PathBuf,
    branch: String,
    path: PathBuf,
    _scratch: TempDir,
}

impl Worktree {
    /// The working directory holding the materialized source.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The branch built in this worktree.
    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Run `git <args>` in the worktree, returning stdout as UTF-8.
    pub fn git(&self, args: &[&str]) -> Result<String> {
        git(&self.path, args)
    }

    /// Tear down the worktree; the branch persists in `repo`.
    pub fn remove(self) -> Result<()> {
        let p = path_str(&self.path)?;
        git(&self.repo, &["worktree", "remove", "--force", p])
            .with_context(|| format!("removing the scratch worktree for {}", self.branch))?;
        Ok(())
    }

    /// Fail-loud cleanup when a post-build step fails: abort any in-progress
    /// `git am`, remove the worktree, delete the partial branch, and delete
    /// `tag` if one was already created — so a half-materialized ref or tag is
    /// never left behind (design §8). Best-effort: every step is attempted even
    /// if an earlier one fails.
    pub fn cleanup_failed(self, tag: Option<&str>) {
        let _ = git(&self.path, &["am", "--abort"]);
        if let Ok(p) = path_str(&self.path) {
            let _ = git(&self.repo, &["worktree", "remove", "--force", p]);
        }
        let _ = git(&self.repo, &["branch", "-D", &self.branch]);
        if let Some(tag) = tag {
            let _ = git(&self.repo, &["tag", "-d", tag]);
        }
    }
}

/// Build the linear `release/<name>` branch in `repo` (design §8) and **return
/// the live worktree** so the caller can append the signed `000-RELEASE/` bundle
/// commit and the annotated tag before tearing it down (see [`crate::bundle`]).
/// Creates an isolated working copy at `base_ref`, then `git am` each patch in
/// `order`, amending a `Crt-Patch: sha256:<blob_hash>` trailer onto each
/// resulting commit. There is **no** `Crt-Visibility` trailer — visibility is
/// store-only, so the branch leaks no classification.
///
/// Fail-loud (design §8): any apply failure aborts the in-progress `am`, tears
/// down the working copy, deletes the partial `branch`, and returns the error —
/// a half-materialized ref is never left behind. On success the worktree is left
/// in place at the last applied commit and the **caller owns its teardown**
/// ([`Worktree::remove`] on success, [`Worktree::cleanup_failed`] on a later
/// failure). Returns the worktree and the patch commit hashes in apply order;
/// the hashes are for inspection only — the bundle BOM is rebuilt from the
/// `Crt-Patch` trailers, not from these carried-forward hashes.
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
) -> Result<(Worktree, Vec<String>)> {
    // `git worktree add` requires its target path not to pre-exist, so create a
    // unique parent dir and join a child the parent does not create. The
    // `TempDir` is the cleanup backstop should a panic skip the explicit
    // teardown the caller performs via the returned `Worktree`.
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

    let wt = Worktree {
        repo: repo.to_path_buf(),
        branch: branch.to_owned(),
        path: work,
        _scratch: parent,
    };

    match apply_patches(&wt.path, patches) {
        Ok(commits) => Ok((wt, commits)),
        Err(e) => {
            // Fail loud: leave no half-materialized ref behind.
            wt.cleanup_failed(None);
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

/// Whether a tag named `tag` already exists in `repo`. `git tag --list <tag>`
/// prints the name when present and nothing otherwise (exit 0 either way), so a
/// non-empty result means the tag exists. Release names carry no glob
/// metacharacters, so the literal name is matched exactly.
pub fn tag_exists(repo: &Path, tag: &str) -> Result<bool> {
    Ok(!git(repo, &["tag", "--list", tag])?.trim().is_empty())
}

/// Check `refname` out into a fresh **detached** scratch worktree for read-only
/// inspection (e.g. `release verify`'s ref-conditional legs), returning the live
/// [`Worktree`] for the caller to [`Worktree::remove`]. Unlike
/// [`materialize_branch`] it creates no branch. Content filters are disabled
/// (`core.autocrlf=false`) so the checked-out bytes equal the materialize
/// worktree's (and any faithful extraction's) — never `git archive`, which
/// re-applies `.gitattributes` (design §8/§14).
///
/// Subprocess `git`, blocking — offload under an async runtime.
pub fn checkout_detached(repo: &Path, refname: &str) -> Result<Worktree> {
    let parent = tempfile::Builder::new()
        .prefix("crt-verify-")
        .tempdir()
        .context("creating the verification scratch directory")?;
    let work = parent.path().join("tree");
    let work_str = path_str(&work)?;
    git(
        repo,
        &[
            "-c",
            "core.autocrlf=false",
            "worktree",
            "add",
            "--detach",
            work_str,
            refname,
        ],
    )
    .with_context(|| format!("checking out {refname} for verification"))?;
    Ok(Worktree {
        repo: repo.to_path_buf(),
        branch: refname.to_owned(),
        path: work,
        _scratch: parent,
    })
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

        let (wt, commits) = materialize_branch(repo, "release/ces-v1", "main", &patches).unwrap();
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

        // The worktree is now the caller's to tear down; once removed, only the
        // main working copy remains and the branch persists in the repo.
        wt.remove().unwrap();
        assert_eq!(
            run(repo, &["worktree", "list"]).lines().count(),
            1,
            "the scratch working copy was not removed"
        );
        assert!(
            git(repo, &["rev-parse", "--verify", "release/ces-v1"]).is_ok(),
            "the branch must persist after the worktree is removed"
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
