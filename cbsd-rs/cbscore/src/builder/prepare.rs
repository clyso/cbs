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

//! Component preparation — stage 1 of the builder pipeline (design 007).
//! Source: `cbscore/builder/prepare.py`, `cbscore/builder/utils.py`.
//!
//! For each component pinned in the version, in parallel: resolve its repo URL
//! against the configured secrets, mirror-clone it, check the ref out into a
//! worktree, apply the version-appropriate patch set, and record its `sha1` and
//! long version. The fan-out is **fail-fast** (the first component error aborts
//! the rest, mirroring Python's `TaskGroup`); worktrees created by components
//! that already finished are cleaned up. On success the caller owns cleanup via
//! [`PreparedComponents::cleanup`] (the async equivalent of Python's
//! `asynccontextmanager` exit — `Drop` cannot be async).

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::builder::BuilderError;
use crate::components::CoreComponentLoc;
use crate::types::VersionComponent;
use crate::types::tracing_targets::BUILDER;
use crate::utils::git::{git_apply, git_checkout, git_clone, git_get_sha1, git_remove_worktree};
use crate::utils::redact::CmdArg;
use crate::utils::secrets::SecretsMgr;
use crate::utils::subprocess::{RunOpts, run_cmd};
use crate::versions::parse::{get_major_version, get_minor_version};

/// Everything the later stages need about one prepared component (`prepare.py`
/// `BuildComponentInfo`). Build-time only — the paths are local to the scratch
/// mount, so this is not a wire type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildComponentInfo {
    pub name: String,
    pub repo_path: Utf8PathBuf,
    pub worktree_path: Utf8PathBuf,
    pub repo_url: String,
    pub base_ref: String,
    pub sha1: String,
    pub long_version: String,
}

/// The prepared components, keyed by name. The caller must [`cleanup`] the
/// worktrees once the build has consumed them (success path); the prepare
/// fan-out already cleans up after itself on failure.
///
/// [`cleanup`]: PreparedComponents::cleanup
#[derive(Debug)]
pub struct PreparedComponents {
    infos: BTreeMap<String, BuildComponentInfo>,
}

impl PreparedComponents {
    /// The prepared components by name.
    pub fn infos(&self) -> &BTreeMap<String, BuildComponentInfo> {
        &self.infos
    }

    /// Remove every prepared component's worktree (best-effort; a failure is
    /// logged, not raised — `cleanup_components` in `prepare.py`).
    pub async fn cleanup(&self) {
        cleanup_worktrees(&self.infos).await;
    }
}

/// Prepare all `components` under `scratch_path`, in parallel and fail-fast.
///
/// `secrets` and `components_loc` are shared across the per-component tasks, so
/// they arrive behind an [`Arc`]. Returns the prepared components on success;
/// the caller then drives the build and calls
/// [`PreparedComponents::cleanup`].
pub async fn prepare_components(
    secrets: Arc<SecretsMgr>,
    scratch_path: &Utf8Path,
    components_loc: Arc<BTreeMap<String, CoreComponentLoc>>,
    components: &[VersionComponent],
    version: &str,
) -> Result<PreparedComponents, BuilderError> {
    let git_repos = scratch_path.join("git").join("repos");
    let git_worktrees = scratch_path.join("git").join("worktrees");
    mkdir_all(&git_repos).await?;
    mkdir_all(&git_worktrees).await?;

    // Every component pinned in the version must have a core definition.
    for comp in components {
        if !components_loc.contains_key(&comp.name) {
            return Err(BuilderError::ComponentNotDefined(comp.name.clone()));
        }
    }

    let mut set: JoinSet<Result<BuildComponentInfo, BuilderError>> = JoinSet::new();
    for comp in components {
        let secrets = Arc::clone(&secrets);
        let components_loc = Arc::clone(&components_loc);
        let git_repos = git_repos.clone();
        let git_worktrees = git_worktrees.clone();
        let version = version.to_string();
        let comp = comp.clone();
        set.spawn(async move {
            do_component(
                &secrets,
                &components_loc,
                &git_repos,
                &git_worktrees,
                &version,
                &comp,
            )
            .await
        });
    }

    let mut infos: BTreeMap<String, BuildComponentInfo> = BTreeMap::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(info)) => {
                infos.insert(info.name.clone(), info);
            }
            Ok(Err(err)) => {
                // Fail-fast: abort the rest and clean up what already landed.
                set.abort_all();
                cleanup_worktrees(&infos).await;
                return Err(err);
            }
            Err(join_err) => {
                set.abort_all();
                cleanup_worktrees(&infos).await;
                return Err(BuilderError::ComponentTaskFailed(join_err.to_string()));
            }
        }
    }

    Ok(PreparedComponents { infos })
}

/// Prepare one component: clone → checkout → apply patches → record info. On a
/// failure after the worktree exists, the worktree is removed before the error
/// propagates (`_do_component` in `prepare.py`).
async fn do_component(
    secrets: &SecretsMgr,
    components_loc: &BTreeMap<String, CoreComponentLoc>,
    git_repos: &Utf8Path,
    git_worktrees: &Utf8Path,
    version: &str,
    comp: &VersionComponent,
) -> Result<BuildComponentInfo, BuilderError> {
    debug!(
        target: BUILDER,
        "prepare component '{}' (repo '{}', ref '{}')", comp.name, comp.repo, comp.git_ref
    );

    // The resolved URL (and any temporary SSH key) lives only across the clone,
    // exactly as Python scopes the `with secrets.git_url_for(...)` block.
    let repo_path = {
        let resolved =
            secrets
                .git_url_for(&comp.repo)
                .await
                .map_err(|source| BuilderError::Secrets {
                    component: comp.name.clone(),
                    source,
                })?;
        clone(resolved.arg(), git_repos, &comp.name).await?
    };

    let worktree_base = git_worktrees.join(&comp.name);
    let worktree_path = git_checkout(&repo_path, &comp.git_ref, &worktree_base)
        .await
        .map_err(|source| BuilderError::Git {
            component: comp.name.clone(),
            source,
        })?;

    match finalize(components_loc, comp, &repo_path, &worktree_path, version).await {
        Ok(info) => Ok(info),
        Err(err) => {
            // Best-effort worktree removal before propagating (prepare.py:379).
            let _ = git_remove_worktree(&repo_path, &worktree_path).await;
            Err(err)
        }
    }
}

/// Clone (mapping the git error to the component context).
async fn clone(
    repo: &CmdArg,
    git_repos: &Utf8Path,
    name: &str,
) -> Result<Utf8PathBuf, BuilderError> {
    git_clone(repo.clone(), git_repos, name)
        .await
        .map_err(|source| BuilderError::Git {
            component: name.to_string(),
            source,
        })
}

/// Apply patches and record `sha1` + long version into a [`BuildComponentInfo`].
async fn finalize(
    components_loc: &BTreeMap<String, CoreComponentLoc>,
    comp: &VersionComponent,
    repo_path: &Utf8Path,
    worktree_path: &Utf8Path,
    version: &str,
) -> Result<BuildComponentInfo, BuilderError> {
    let comp_loc = &components_loc[&comp.name];
    apply_patches(comp_loc, comp, worktree_path, version).await?;

    let sha1 = git_get_sha1(worktree_path)
        .await
        .map_err(|source| BuilderError::Git {
            component: comp.name.clone(),
            source,
        })?;
    let long_version = get_component_version(comp_loc, worktree_path).await?;

    Ok(BuildComponentInfo {
        name: comp.name.clone(),
        repo_path: repo_path.to_owned(),
        worktree_path: worktree_path.to_owned(),
        repo_url: comp.repo.clone(),
        base_ref: comp.git_ref.clone(),
        sha1,
        long_version,
    })
}

/// Apply the version-appropriate patch set to the worktree. A missing component
/// directory or `patches/` directory is a no-op (`_apply_patches` in
/// `prepare.py`).
async fn apply_patches(
    comp_loc: &CoreComponentLoc,
    comp: &VersionComponent,
    worktree_path: &Utf8Path,
    version: &str,
) -> Result<(), BuilderError> {
    if !comp_loc.path.exists() {
        warn!(target: BUILDER, "component '{}' not found at '{}'", comp.name, comp_loc.path);
        return Ok(());
    }
    let patches_path = comp_loc.path.join("patches");
    if !patches_path.exists() {
        info!(target: BUILDER, "no patches to apply to '{}'", comp.name);
        return Ok(());
    }
    // git_apply runs under `-C worktree`, so the patch paths must be absolute.
    let patches_path = patches_path
        .canonicalize_utf8()
        .map_err(|source| io_err(format!("resolving '{patches_path}'"), source))?;

    for patch in get_patch_list(&patches_path, version)? {
        info!(target: BUILDER, "applying patch '{patch}' to '{}'", comp.name);
        git_apply(worktree_path, &patch)
            .await
            .map_err(|source| BuilderError::Git {
                component: comp.name.clone(),
                source,
            })?;
    }
    Ok(())
}

/// Run a component's `get_version` script (with the worktree as cwd) and return
/// its trimmed stdout (`get_component_version` in `builder/utils.py`).
async fn get_component_version(
    comp_loc: &CoreComponentLoc,
    worktree_path: &Utf8Path,
) -> Result<String, BuilderError> {
    let name = &comp_loc.comp.name;
    let script = comp_loc.path.join(&comp_loc.comp.build.get_version);
    if !script.exists() {
        return Err(BuilderError::MissingScript {
            component: name.clone(),
            script: "get_version".to_string(),
            path: script,
        });
    }
    // The script runs with the worktree as cwd, so resolve it to an absolute
    // path first (Python's `resolve()`).
    let script = script
        .canonicalize_utf8()
        .map_err(|source| io_err(format!("resolving '{script}'"), source))?;

    let out = run_cmd(
        &[CmdArg::from(script.as_str())],
        RunOpts {
            cwd: Some(worktree_path),
            ..RunOpts::default()
        },
    )
    .await
    .map_err(|source| BuilderError::Command {
        context: format!("running get_version for '{name}'"),
        source,
    })?;
    if out.code != 0 {
        return Err(BuilderError::Step {
            context: format!("get_version for '{name}'"),
            code: out.code,
            stderr: out.stderr,
        });
    }
    Ok(out.stdout.trim().to_string())
}

/// A `NNNN-...patch` filename, capturing the leading numeric prefix
/// (`prepare.py:130`).
static PATCH_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+)-.*\.patch").expect("patch prefix regex is valid"));

/// Select and order a component's patches for `version` (`_get_patch_list` in
/// `prepare.py`). Patches live directly under `patches/`, ordered by their
/// numeric prefix; nested directories named for the exact / minor (`M.m.p`) /
/// major (`M.m`) version contribute version-specific sets, and the deepest
/// directories are applied first.
fn get_patch_list(
    patches_path: &Utf8Path,
    version: &str,
) -> Result<Vec<Utf8PathBuf>, BuilderError> {
    debug!(target: BUILDER, "get patch list for version '{version}'");

    // The directory-name selectors. A malformed `version` cannot reach here (it
    // is validated upstream); should one slip through, the major/minor tiers
    // simply do not match rather than erroring (`.ok()`), and exact-name dirs
    // still work.
    let exact = version;
    let minor = get_minor_version(version).ok().flatten();
    let major = get_major_version(version).ok();

    let mut by_depth: BTreeMap<usize, Vec<(u64, Utf8PathBuf)>> = BTreeMap::new();
    collect_patches(
        patches_path,
        0,
        exact,
        minor.as_deref(),
        major.as_deref(),
        &mut by_depth,
    )?;

    // Deepest directory first; within a depth, ascending numeric prefix.
    let mut ordered = Vec::new();
    for (_, mut entries) in by_depth.into_iter().rev() {
        entries.sort_by_key(|(num, _)| *num);
        ordered.extend(entries.into_iter().map(|(_, path)| path));
    }
    Ok(ordered)
}

/// Recursively gather patches by directory depth, pruning version-named
/// subdirectories that do not match `version`.
fn collect_patches(
    path: &Utf8Path,
    depth: usize,
    exact: &str,
    minor: Option<&str>,
    major: Option<&str>,
    by_depth: &mut BTreeMap<usize, Vec<(u64, Utf8PathBuf)>>,
) -> Result<(), BuilderError> {
    if depth > 0 {
        let name = path.file_name().unwrap_or_default();
        let matches = name == exact || Some(name) == minor || Some(name) == major;
        if !matches {
            debug!(target: BUILDER, "patch dir '{name}' does not match version selectors");
            return Ok(());
        }
    }
    by_depth.entry(depth).or_default();

    for entry in read_dir_sorted(path)? {
        if entry.is_dir() {
            collect_patches(&entry, depth + 1, exact, minor, major, by_depth)?;
        } else if entry.file_name().is_some_and(|n| n.ends_with(".patch")) {
            let name = entry.file_name().unwrap_or_default();
            match PATCH_PREFIX_RE.captures(name) {
                Some(caps) => {
                    let num = caps[1].parse::<u64>().unwrap_or(0);
                    by_depth
                        .get_mut(&depth)
                        .expect("depth inserted above")
                        .push((num, entry));
                }
                None => warn!(target: BUILDER, "patch name '{name}' malformed; skipping"),
            }
        }
        // A non-`.patch` plain file is ignored. Python recurses into every
        // non-`.patch` entry, but its depth>0 version-name guard prunes entries
        // whose name isn't the exact/minor/major version — so common strays
        // (`README`, `series`) are skipped by both. Only a stray file named
        // exactly like a version selector makes Python `iterdir` a non-directory
        // and raise `NotADirectoryError`; the port skips it. Robust, and real
        // patch trees hold only `.patch` files and version subdirectories.
    }
    Ok(())
}

/// Directory entries as UTF-8 paths, sorted by name for deterministic ordering
/// (`read_dir` order is unspecified; Python relies on filesystem order).
fn read_dir_sorted(dir: &Utf8Path) -> Result<Vec<Utf8PathBuf>, BuilderError> {
    let mut entries = Vec::new();
    for entry in
        std::fs::read_dir(dir).map_err(|source| io_err(format!("reading '{dir}'"), source))?
    {
        let entry =
            entry.map_err(|source| io_err(format!("reading an entry in '{dir}'"), source))?;
        match Utf8PathBuf::from_path_buf(entry.path()) {
            Ok(path) => entries.push(path),
            Err(path) => warn!(target: BUILDER, "skipping non-UTF-8 path '{}'", path.display()),
        }
    }
    entries.sort();
    Ok(entries)
}

/// Remove every component's worktree, logging (not raising) on failure.
async fn cleanup_worktrees(infos: &BTreeMap<String, BuildComponentInfo>) {
    for (name, info) in infos {
        if let Err(err) = git_remove_worktree(&info.repo_path, &info.worktree_path).await {
            warn!(
                target: BUILDER,
                "unable to clean up component '{name}' worktree '{}': {err}", info.worktree_path
            );
        }
    }
}

async fn mkdir_all(path: &Utf8Path) -> Result<(), BuilderError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|source| io_err(format!("creating '{path}'"), source))
}

fn io_err(context: String, source: std::io::Error) -> BuilderError {
    BuilderError::Io { context, source }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    use crate::builder::test_support::retry_spawn;
    use crate::components::{
        CoreComponent, CoreComponentBuild, CoreComponentContainers, CoreComponentLoc,
    };
    use crate::types::Secrets;

    fn empty_secrets() -> Arc<SecretsMgr> {
        Arc::new(SecretsMgr::new(Secrets {
            schema_version: 1,
            git: BTreeMap::new(),
            storage: BTreeMap::new(),
            sign: BTreeMap::new(),
            registry: BTreeMap::new(),
        }))
    }

    fn write(path: &Utf8Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    // ----- _get_patch_list ----------------------------------------------

    fn touch_patch(dir: &Utf8Path, name: &str) {
        write(&dir.join(name), "patch\n");
    }

    #[test]
    fn get_patch_list_orders_deepest_first_then_by_numeric_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let patches = Utf8Path::from_path(tmp.path()).unwrap().join("patches");

        // Root patches (depth 0).
        touch_patch(&patches, "0002-root-b.patch");
        touch_patch(&patches, "0001-root-a.patch");
        // Major-version dir (depth 1).
        touch_patch(&patches.join("1.2"), "0001-major.patch");
        // Exact/minor-version dir (depth 1).
        touch_patch(&patches.join("1.2.3"), "0005-exact-e.patch");
        touch_patch(&patches.join("1.2.3"), "0001-exact-a.patch");
        // Non-matching version dir (pruned).
        touch_patch(&patches.join("9.9.9"), "0001-nope.patch");
        // A malformed name (warned + skipped).
        touch_patch(&patches, "nodigits.patch");

        let list = get_patch_list(&patches, "1.2.3").unwrap();
        let names: Vec<&str> = list.iter().map(|p| p.file_name().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                // depth 1 first, ascending prefix; the "1.2" dir sorts before
                // "1.2.3", so its 0001 leads the 0001 from "1.2.3".
                "0001-major.patch",
                "0001-exact-a.patch",
                "0005-exact-e.patch",
                // depth 0 last.
                "0001-root-a.patch",
                "0002-root-b.patch",
            ]
        );
        // The non-matching version dir contributed nothing.
        assert!(!list.iter().any(|p| p.as_str().contains("9.9.9")));
    }

    // ----- get_component_version ----------------------------------------

    /// A component dir with an executable `get_version` script printing
    /// `version`.
    fn component_loc_with_version_script(
        dir: &Utf8Path,
        name: &str,
        version: &str,
    ) -> CoreComponentLoc {
        let script = dir.join("get_version.sh");
        write(&script, &format!("#!/bin/sh\necho {version}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        CoreComponentLoc {
            path: dir.to_owned(),
            comp: CoreComponent {
                name: name.to_string(),
                repo: "https://github.com/ceph/ceph".to_string(),
                build: CoreComponentBuild {
                    rpm: None,
                    get_version: "get_version.sh".to_string(),
                    deps: "deps.sh".to_string(),
                },
                containers: CoreComponentContainers {
                    path: "containers".into(),
                },
            },
        }
    }

    #[tokio::test]
    async fn get_component_version_runs_the_script_in_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("component");
        std::fs::create_dir_all(&comp_dir).unwrap();
        let loc = component_loc_with_version_script(&comp_dir, "ceph", "20.2.1-custom");

        let worktree = base.join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();

        let version = retry_spawn(|| get_component_version(&loc, &worktree)).await;
        assert_eq!(version, "20.2.1-custom");
    }

    #[tokio::test]
    async fn get_component_version_errors_when_the_script_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let loc = CoreComponentLoc {
            path: base.to_owned(),
            comp: CoreComponent {
                name: "ceph".to_string(),
                repo: "https://github.com/ceph/ceph".to_string(),
                build: CoreComponentBuild {
                    rpm: None,
                    get_version: "absent.sh".to_string(),
                    deps: "deps.sh".to_string(),
                },
                containers: CoreComponentContainers {
                    path: "containers".into(),
                },
            },
        };
        let err = get_component_version(&loc, base).await.unwrap_err();
        assert!(matches!(err, BuilderError::MissingScript { .. }), "{err}");
    }

    // ----- prepare_components -------------------------------------------

    async fn git(dir: &Utf8Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .await
            .expect("git available");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Create a source repo at `dir` with a README on a `testref` branch; return
    /// its HEAD sha.
    async fn init_source(dir: &Utf8Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]).await;
        git(dir, &["config", "user.name", "Test"]).await;
        git(dir, &["config", "user.email", "test@example.com"]).await;
        std::fs::write(dir.join("README"), "hello\n").unwrap();
        git(dir, &["add", "."]).await;
        git(dir, &["commit", "-q", "-m", "init"]).await;
        git(dir, &["branch", "testref"]).await;
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    const README_PATCH: &str = "--- a/README\n+++ b/README\n@@ -1 +1,2 @@\n hello\n+patched\n";

    #[tokio::test]
    async fn prepare_components_clones_checks_out_patches_and_records_info() {
        // A dot-free base so the `file://` URL validates (the git-URL grammar's
        // path class excludes '.').
        let tmp = tempfile::Builder::new()
            .prefix("cbsprep")
            .tempdir()
            .unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();

        // A source repo to clone.
        let source = base.join("source");
        let head = init_source(&source).await;
        let repo_url = format!("file://{source}");

        // A component definition dir with a get_version script and one patch.
        let comp_dir = base.join("components").join("ceph");
        let loc = component_loc_with_version_script(&comp_dir, "ceph", "1.2.3-build");
        write(
            &comp_dir.join("patches").join("0001-readme.patch"),
            README_PATCH,
        );

        let components_loc = Arc::new(BTreeMap::from([("ceph".to_string(), loc)]));
        let components = vec![VersionComponent {
            name: "ceph".to_string(),
            repo: repo_url.clone(),
            git_ref: "testref".to_string(),
        }];

        let scratch = base.join("scratch");
        let prepared = retry_spawn(|| {
            prepare_components(
                empty_secrets(),
                &scratch,
                Arc::clone(&components_loc),
                &components,
                "1.2.3",
            )
        })
        .await;

        let info = &prepared.infos()["ceph"];
        assert_eq!(info.name, "ceph");
        assert_eq!(info.sha1, head);
        assert_eq!(info.long_version, "1.2.3-build");
        assert_eq!(info.repo_url, repo_url);
        assert_eq!(info.base_ref, "testref");
        // The patch was applied in the worktree.
        let readme = std::fs::read_to_string(info.worktree_path.join("README")).unwrap();
        assert!(readme.contains("patched"), "patch applied: {readme}");

        // Cleanup removes the worktree.
        let worktree = info.worktree_path.clone();
        prepared.cleanup().await;
        assert!(!worktree.exists(), "worktree removed by cleanup");
    }

    #[tokio::test]
    async fn prepare_components_rejects_a_component_without_a_definition() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = Utf8Path::from_path(tmp.path()).unwrap().join("scratch");
        let components = vec![VersionComponent {
            name: "missing".to_string(),
            repo: "https://github.com/ceph/ceph".to_string(),
            git_ref: "main".to_string(),
        }];

        let err = prepare_components(
            empty_secrets(),
            &scratch,
            Arc::new(BTreeMap::new()),
            &components,
            "1.2.3",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, BuilderError::ComponentNotDefined(name) if name == "missing"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn prepare_components_fails_fast_when_a_clone_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("components").join("ceph");
        let loc = component_loc_with_version_script(&comp_dir, "ceph", "1.2.3");
        let components_loc = Arc::new(BTreeMap::from([("ceph".to_string(), loc)]));

        // A valid-but-nonexistent file:// repo: the URL validates (no match →
        // unchanged), but the clone fails.
        let components = vec![VersionComponent {
            name: "ceph".to_string(),
            repo: "file:///nonexistent-cbs-prepare-test/src".to_string(),
            git_ref: "main".to_string(),
        }];

        let scratch = base.join("scratch");
        let err = prepare_components(
            empty_secrets(),
            &scratch,
            components_loc,
            &components,
            "1.2.3",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, BuilderError::Git { component, .. } if component == "ceph"),
            "{err}"
        );
    }
}
