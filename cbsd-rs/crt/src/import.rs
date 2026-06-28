// crt — `patch import` from a local git range or a GitHub PR.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ingest patches into the content-addressed store (design §4, plan M1.1 /
//! M1.3). Patch bytes always come from a local `git format-patch` (never
//! GitHub's `.patch` endpoint, design §4); `patch_id` is `git patch-id
//! --stable`. A matching `patch_id` already in the store flags the import as
//! equivalent to an existing blob so the operator reuses it rather than
//! storing a near-duplicate (design §4).

use std::path::Path;

use anyhow::{Context, Result, bail};
use crt_core::{
    Identity, PatchMeta, Provenance, Sha256, UpstreamPrState, blob_hash, cherry_picked_from,
};
use crt_store::Store;

use crate::git;

/// One imported patch, for CLI reporting.
pub struct Imported {
    pub blob_hash: Sha256,
    pub subject: String,
    /// True if the blob was already present (idempotent re-import).
    pub already_present: bool,
    /// Set when an equivalent patch (same `patch_id`, different bytes) is
    /// already stored — the operator should reuse that blob (design §4).
    pub equivalent_to: Option<Sha256>,
}

/// Import every non-merge commit in `range` (e.g. `A..B`) from the repo at
/// `repo`, content-addressing each into `store` with visibility-neutral
/// `PatchMeta`.
pub async fn import_range(
    store: &dyn Store,
    repo: &Path,
    range: &str,
    source_repo: &str,
) -> Result<Vec<Imported>> {
    // `--no-merges`: merge commits have no single-parent diff to format-patch.
    let revs = git::git(repo, &["rev-list", "--reverse", "--no-merges", range])
        .with_context(|| format!("listing commits in {range}"))?;
    let shas: Vec<String> = revs.split_whitespace().map(str::to_owned).collect();
    let provenance = Provenance::Other {
        description: format!("{source_repo} {range}"),
    };
    import_shas(store, repo, &shas, &provenance, source_repo).await
}

/// GitHub returns at most this many commits for one PR via the PR-commits
/// endpoint; larger PRs are truncated (the operator should fall back to
/// `--range`). See the GitHub REST "List commits on a pull request" docs.
const PR_COMMIT_LIST_CAP: usize = 250;

/// Import the commits of a GitHub PR. The commit list and PR state come from
/// the GitHub API via `octocrab`; the patch bytes always come from a local
/// `git format-patch` after fetching the PR head into `repo` (never GitHub's
/// `.patch` endpoint, design §4).
///
/// `token` authenticates both the GitHub **API** calls (via `octocrab`) and the
/// local `git fetch` of the PR head (via `git::fetch_github_ref`), so PRs on
/// **private** repositories are supported. The token needs `Contents: Read` on
/// the repository for the fetch, not just metadata/PR read.
pub async fn import_pr(
    store: &dyn Store,
    repo: &Path,
    pr_url: &str,
    token: Option<&str>,
) -> Result<Vec<Imported>> {
    let (owner, name, number) = parse_pr_url(pr_url)?;

    let mut builder = octocrab::Octocrab::builder();
    if let Some(token) = token {
        builder = builder.personal_token(token.to_owned());
    }
    let gh = builder.build().context("building the GitHub client")?;

    let pr = gh
        .pulls(&owner, &name)
        .get(number)
        .await
        .with_context(|| format!("fetching {owner}/{name}#{number}"))?;
    let merged = pr.merged_at.is_some();
    let closed = matches!(pr.state, Some(octocrab::models::IssueState::Closed));
    let base_ref = pr
        .base
        .as_ref()
        .map(|b| b.ref_field.clone())
        .context("PR metadata has no base ref")?;
    let head_sha = pr
        .head
        .as_ref()
        .map(|h| h.sha.clone())
        .context("PR metadata has no head")?;
    let state = pr_state(merged, closed, &base_ref);
    let html_url = pr
        .html_url
        .map_or_else(|| pr_url.to_owned(), |u| u.to_string());

    // Enumerate the PR's commits from GitHub — the authoritative set, robust to
    // merge style. A `base..head` range is not: a merged PR's reported base tip
    // is unreliable, so the range can resolve empty or include base commits
    // (F1). Merge commits within the PR have no single-parent diff, so drop them
    // by parent count.
    let first = gh
        .pulls(&owner, &name)
        .pr_commits(number)
        .per_page(100u8)
        .send()
        .await
        .with_context(|| format!("listing commits of {owner}/{name}#{number}"))?;
    let commits = gh
        .all_pages(first)
        .await
        .with_context(|| format!("paging commits of {owner}/{name}#{number}"))?;
    if commits.len() >= PR_COMMIT_LIST_CAP {
        eprintln!(
            "warning: {owner}/{name}#{number} reports {} commits, at or above the \
             GitHub per-PR commit-list cap of {PR_COMMIT_LIST_CAP}; some commits may \
             be missing — re-run with --range for a complete import",
            commits.len()
        );
    }
    let shas: Vec<String> = commits
        .into_iter()
        .filter(|c| c.parents.len() <= 1)
        .map(|c| c.sha)
        .collect();
    if shas.is_empty() {
        bail!("{owner}/{name}#{number} has no non-merge commits to import");
    }

    // Fetch the PR head so the enumerated commits are available locally. The
    // token authenticates this `git` fetch too (off-argv, see
    // `git::fetch_github_ref`), so PR heads on private repositories work.
    git::fetch_github_ref(repo, &owner, &name, &format!("pull/{number}/head"), token)
        .with_context(|| format!("fetching pull/{number}/head for {owner}/{name}"))?;

    let provenance = Provenance::UpstreamPr {
        prs: vec![html_url],
        commits: vec![head_sha],
        state,
    };
    let source_repo = format!("{owner}/{name}");
    import_shas(store, repo, &shas, &provenance, &source_repo).await
}

/// Parse `https://github.com/<owner>/<repo>/pull/<number>` (trailing path
/// segments such as `/files` are ignored).
fn parse_pr_url(url: &str) -> Result<(String, String, u64)> {
    let rest = url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| url.trim().strip_prefix("http://github.com/"))
        .with_context(|| format!("not a github.com URL: {url}"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 4 || parts[2] != "pull" {
        bail!("expected https://github.com/<owner>/<repo>/pull/<n>, got: {url}");
    }
    let owner = parts[0].to_owned();
    let name = parts[1].to_owned();
    if owner.is_empty() || name.is_empty() {
        bail!("empty owner or repo in PR URL: {url}");
    }
    let number: u64 = parts[3]
        .parse()
        .with_context(|| format!("PR number in {url}"))?;
    Ok((owner, name, number))
}

/// Map a PR's status and base branch to an `UpstreamPrState`. Best effort:
/// `main`/`master` ⇒ "main", any other base ⇒ a stable branch; a closed but
/// unmerged PR is `Declined`; an open PR is recorded as in-review (approval
/// state is not fetched in M1).
fn pr_state(merged: bool, closed: bool, base_ref: &str) -> UpstreamPrState {
    match (merged, closed, base_ref) {
        (true, _, "main" | "master") => UpstreamPrState::MergedMain,
        (true, _, _) => UpstreamPrState::MergedStable,
        (false, true, _) => UpstreamPrState::Declined,
        (false, false, _) => UpstreamPrState::OpenInReview,
    }
}

/// Import each commit in `shas` (already in apply order) under a shared
/// `provenance`. Callers pre-filter merge commits.
async fn import_shas(
    store: &dyn Store,
    repo: &Path,
    shas: &[String],
    provenance: &Provenance,
    source_repo: &str,
) -> Result<Vec<Imported>> {
    let mut out = Vec::with_capacity(shas.len());
    for sha in shas {
        out.push(import_commit(store, repo, sha, provenance, source_repo).await?);
    }
    Ok(out)
}

async fn import_commit(
    store: &dyn Store,
    repo: &Path,
    sha: &str,
    provenance: &Provenance,
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
        bail!("empty patch for {sha}");
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

    // An existing blob under the same `patch_id` but a *different* `blob_hash`
    // is an equivalent change re-exported with different bytes (design §4).
    let existing = store.get_patch_id(&patch_id).await?;
    let equivalent_to = existing.filter(|e| *e != hash);

    let meta = PatchMeta {
        blob_hash: hash,
        patch_id: patch_id.clone(),
        author: Identity { name, email },
        authored,
        subject: subject.clone(),
        body: body.clone(),
        cherry_picked_from: cherry_picked_from(&body),
        provenance: provenance.clone(),
        source_repo: source_repo.to_owned(),
    };

    // Blob then meta: a partial failure can leave a blob without meta; a
    // re-import heals the meta (reporting `already_present`). Acceptable for
    // the CLI — the future service can make this transactional.
    let already_present = store.has_blob(&hash).await?;
    store.put_blob(&hash, &blob).await?;
    store.put_meta(&hash, &meta).await?;
    // Record the first blob seen for this `patch_id` as its representative.
    if existing.is_none() {
        store.put_patch_id(&patch_id, &hash).await?;
    }

    Ok(Imported {
        blob_hash: hash,
        subject,
        already_present,
        equivalent_to,
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

    fn rev_parse(repo: &Path, rev: &str) -> String {
        git::git(repo, &["rev-parse", rev])
            .expect("rev-parse")
            .trim()
            .to_owned()
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
        assert!(imported.iter().all(|p| p.equivalent_to.is_none()));

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
        assert!(again.iter().all(|p| p.equivalent_to.is_none()));
    }

    #[tokio::test]
    async fn equivalent_patch_id_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["config", "user.name", "Tester"]);
        run_git(repo, &["config", "user.email", "tester@example.com"]);
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "base"]);
        let base = rev_parse(repo, "HEAD");

        // Two commits with an identical diff but different messages ⇒ same
        // patch_id, different blob_hash (the re-exported / rebased case).
        std::fs::write(repo.join("a.txt"), "base\nNEW\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "alpha"]);
        let alpha = rev_parse(repo, "HEAD");

        run_git(repo, &["reset", "--hard", &base]);
        std::fs::write(repo.join("a.txt"), "base\nNEW\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(
            repo,
            &["commit", "-q", "-m", "beta: a different commit message"],
        );
        let beta = rev_parse(repo, "HEAD");

        let store = ObjectBackedStore::in_memory();
        let first = import_range(&store, repo, &format!("{base}..{alpha}"), "test")
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(first[0].equivalent_to.is_none());

        let second = import_range(&store, repo, &format!("{base}..{beta}"), "test")
            .await
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].blob_hash, first[0].blob_hash);
        assert_eq!(second[0].equivalent_to, Some(first[0].blob_hash));
    }

    #[test]
    fn parses_pr_urls() {
        assert_eq!(
            parse_pr_url("https://github.com/ceph/ceph/pull/12345").unwrap(),
            ("ceph".to_owned(), "ceph".to_owned(), 12345)
        );
        assert_eq!(
            parse_pr_url("https://github.com/clyso/ceph/pull/7/files").unwrap(),
            ("clyso".to_owned(), "ceph".to_owned(), 7)
        );
        assert!(parse_pr_url("https://gitlab.com/x/y/pull/1").is_err());
        assert!(parse_pr_url("https://github.com/ceph/ceph/issues/3").is_err());
        assert!(parse_pr_url("https://github.com/ceph/ceph/pull/nope").is_err());
    }

    #[test]
    fn pr_state_mapping() {
        assert_eq!(pr_state(true, false, "main"), UpstreamPrState::MergedMain);
        assert_eq!(pr_state(true, false, "master"), UpstreamPrState::MergedMain);
        // A merged PR is merged even if the API also marks it closed.
        assert_eq!(pr_state(true, true, "main"), UpstreamPrState::MergedMain);
        assert_eq!(
            pr_state(true, false, "squid"),
            UpstreamPrState::MergedStable
        );
        assert_eq!(pr_state(false, true, "main"), UpstreamPrState::Declined);
        assert_eq!(
            pr_state(false, false, "main"),
            UpstreamPrState::OpenInReview
        );
    }

    #[tokio::test]
    async fn import_shas_records_pr_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        fixture_repo(repo);
        let store = ObjectBackedStore::in_memory();

        // The two patch commits, oldest-first — the shape `import_pr` feeds in.
        let revs = git::git(repo, &["rev-list", "--reverse", "HEAD~2..HEAD"]).unwrap();
        let shas: Vec<String> = revs.split_whitespace().map(str::to_owned).collect();
        let provenance = Provenance::UpstreamPr {
            prs: vec!["https://github.com/ceph/ceph/pull/1".to_owned()],
            commits: vec!["deadbeef".to_owned()],
            state: UpstreamPrState::MergedMain,
        };

        let imported = import_shas(&store, repo, &shas, &provenance, "ceph/ceph")
            .await
            .unwrap();
        assert_eq!(imported.len(), 2);
        for p in &imported {
            let meta = store.get_meta(&p.blob_hash).await.unwrap();
            assert_eq!(meta.provenance, provenance);
            assert_eq!(meta.source_repo, "ceph/ceph");
        }
    }

    #[tokio::test]
    async fn range_skips_merge_commits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["config", "user.name", "Tester"]);
        run_git(repo, &["config", "user.email", "tester@example.com"]);
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "base"]);
        let base = rev_parse(repo, "HEAD");

        // A side commit, then a divergent commit on a detached base, then a
        // merge of the two — so `base..HEAD` spans a real merge commit.
        std::fs::write(repo.join("b.txt"), "side\n").unwrap();
        run_git(repo, &["add", "b.txt"]);
        run_git(repo, &["commit", "-q", "-m", "side"]);
        let side = rev_parse(repo, "HEAD");

        run_git(repo, &["checkout", "-q", &base]);
        std::fs::write(repo.join("c.txt"), "mainline\n").unwrap();
        run_git(repo, &["add", "c.txt"]);
        run_git(repo, &["commit", "-q", "-m", "mainline"]);
        run_git(repo, &["merge", "--no-ff", "--no-edit", &side]);
        let merge = rev_parse(repo, "HEAD");

        let store = ObjectBackedStore::in_memory();
        let imported = import_range(&store, repo, &format!("{base}..{merge}"), "test")
            .await
            .unwrap();

        // The two single-parent commits import; the merge commit is skipped.
        assert_eq!(imported.len(), 2);
        let subjects: Vec<_> = imported.iter().map(|p| p.subject.as_str()).collect();
        assert!(subjects.contains(&"side"));
        assert!(subjects.contains(&"mainline"));
        assert!(!subjects.contains(&"merge"));
    }

    /// Real GitHub PR import. Opt-in: set `CRT_TEST_PR_URL` (and optionally
    /// `CRT_TEST_GITHUB_TOKEN`) and run with
    /// `cargo test -p crt -- --ignored`. Clones the PR's repo into a temp dir;
    /// never runs in plain `cargo test` (it needs network).
    #[tokio::test]
    #[ignore = "requires network; set CRT_TEST_PR_URL and run --ignored"]
    async fn import_pr_real() {
        let pr_url = std::env::var("CRT_TEST_PR_URL").expect("CRT_TEST_PR_URL");
        let token = std::env::var("CRT_TEST_GITHUB_TOKEN").ok();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["config", "user.name", "Tester"]);
        run_git(repo, &["config", "user.email", "tester@example.com"]);
        let store = ObjectBackedStore::in_memory();
        let imported = import_pr(&store, repo, &pr_url, token.as_deref())
            .await
            .unwrap();
        assert!(!imported.is_empty(), "PR yielded at least one patch");
    }
}
