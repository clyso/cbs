// crt — `patch import` from a local git range.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ingest patches from a local git range into the content-addressed store
//! (design §4, plan M1.1). Patch bytes come from a local `git format-patch`
//! (never GitHub's `.patch` endpoint); `patch_id` is `git patch-id --stable`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use crt_core::{Identity, PatchMeta, Provenance, Sha256, blob_hash, cherry_picked_from};
use crt_store::Store;

use crate::git;

/// One imported patch, for CLI reporting.
pub struct Imported {
    pub blob_hash: Sha256,
    pub subject: String,
    /// True if the blob was already present (idempotent re-import).
    pub already_present: bool,
}

/// Import every commit in `range` (e.g. `A..B`) from the repo at `repo`,
/// content-addressing each into `store` with visibility-neutral `PatchMeta`.
pub async fn import_range(
    store: &dyn Store,
    repo: &Path,
    range: &str,
    source_repo: &str,
) -> Result<Vec<Imported>> {
    let revs = git::git(repo, &["rev-list", "--reverse", range])
        .with_context(|| format!("listing commits in {range}"))?;
    let mut out = Vec::new();
    for sha in revs.split_whitespace() {
        out.push(import_commit(store, repo, sha, range, source_repo).await?);
    }
    Ok(out)
}

async fn import_commit(
    store: &dyn Store,
    repo: &Path,
    sha: &str,
    range: &str,
    source_repo: &str,
) -> Result<Imported> {
    // Pin format-patch output against ambient `format.*` git config so the
    // `blob_hash` content address is reproducible across environments
    // (design §4): no version signature, no numbering, fixed subject prefix,
    // no auto sign-off.
    let blob = git::git_bytes(
        repo,
        &[
            "-c",
            "format.signoff=false",
            "-c",
            "format.subjectPrefix=PATCH",
            "format-patch",
            "-1",
            "--no-signature",
            "-N",
            "--stdout",
            sha,
        ],
    )?;
    if blob.is_empty() {
        bail!("empty patch for {sha} (merge commit?)");
    }
    let hash = blob_hash(&blob);

    let patch_id = git::git_with_stdin(repo, &["patch-id", "--stable"], &blob)?
        .split_whitespace()
        .next()
        .context("git patch-id produced no id")?
        .to_owned();

    let fields = git::git(repo, &["show", "-s", "--format=%an%n%ae%n%aI%n%s", sha])?;
    let mut lines = fields.lines();
    let name = lines.next().unwrap_or_default().to_owned();
    let email = lines.next().unwrap_or_default().to_owned();
    let authored = lines.next().unwrap_or_default().to_owned();
    let subject = lines.next().unwrap_or_default().to_owned();
    let body = git::git(repo, &["show", "-s", "--format=%b", sha])?;

    let meta = PatchMeta {
        blob_hash: hash,
        patch_id,
        author: Identity { name, email },
        authored,
        subject: subject.clone(),
        body: body.clone(),
        cherry_picked_from: cherry_picked_from(&body),
        provenance: Provenance::Other {
            description: format!("{source_repo} {range}"),
        },
        source_repo: source_repo.to_owned(),
    };

    // Blob then meta: a partial failure can leave a blob without meta; a
    // re-import heals the meta (reporting `already_present`). Acceptable for
    // the CLI — the future service can make this transactional.
    let already_present = store.has_blob(&hash).await?;
    store.put_blob(&hash, &blob).await?;
    store.put_meta(&hash, &meta).await?;
    Ok(Imported {
        blob_hash: hash,
        subject,
        already_present,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt_store::ObjectBackedStore;
    use std::process::Command;

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo)
            .args(args)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Create a 3-commit repo: a base, then two patches to import.
    fn fixture_repo(repo: &Path) {
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["config", "user.name", "Tester"]);
        run_git(repo, &["config", "user.email", "tester@example.com"]);
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "base"]);
        std::fs::write(repo.join("a.txt"), "base\none\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "first"]);
        std::fs::write(repo.join("a.txt"), "base\none\ntwo\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(
            repo,
            &[
                "commit",
                "-q",
                "-m",
                "second\n\n(cherry picked from commit deadbeef)",
            ],
        );
    }

    #[tokio::test]
    async fn imports_a_range_into_the_store() {
        let dir = tempfile::tempdir().unwrap();
        fixture_repo(dir.path());
        let store = ObjectBackedStore::in_memory();

        let imported = import_range(&store, dir.path(), "HEAD~2..HEAD", "test-repo")
            .await
            .unwrap();

        assert_eq!(imported.len(), 2);
        let subjects: Vec<_> = imported.iter().map(|p| p.subject.as_str()).collect();
        assert_eq!(subjects, vec!["first", "second"]);

        for p in &imported {
            assert!(store.has_blob(&p.blob_hash).await.unwrap());
            let meta = store.get_meta(&p.blob_hash).await.unwrap();
            assert_eq!(meta.blob_hash, p.blob_hash);
            assert!(!meta.patch_id.is_empty(), "patch_id captured");
        }

        let second = store.get_meta(&imported[1].blob_hash).await.unwrap();
        assert_eq!(second.cherry_picked_from, vec!["deadbeef"]);

        // blob_hash reproducibility: the version-dependent signature trailer
        // must be stripped from the stored blob (design §4).
        let blob0 = store.get_blob(&imported[0].blob_hash).await.unwrap();
        assert!(
            !String::from_utf8_lossy(&blob0).contains("\n-- \n"),
            "format-patch signature trailer must be stripped"
        );

        // Re-import is idempotent and reported as already present.
        let again = import_range(&store, dir.path(), "HEAD~2..HEAD", "test-repo")
            .await
            .unwrap();
        assert!(again.iter().all(|p| p.already_present));
    }
}
