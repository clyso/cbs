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

//! RPM building — stage 2 of the builder pipeline (design 007). Source:
//! `cbscore/builder/rpmbuild.py`.
//!
//! First installs every component's build dependencies **sequentially** (the
//! `install_deps` scripts mutate shared dnf state), then builds each component's
//! RPMs **in parallel** (fail-fast). Each component gets an `rpmbuild` topdir
//! under `<rpms>/<name>/<version>/` and its `build_rpms` script runs there with
//! the worktree as cwd. `skip_build` lays out the topdir but runs nothing; a
//! configured ccache is passed through as `CES_CCACHE_PATH`.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::builder::BuilderError;
use crate::builder::prepare::BuildComponentInfo;
use crate::components::CoreComponentLoc;
use crate::types::tracing_targets::BUILDER;
use crate::utils::redact::CmdArg;
use crate::utils::subprocess::{OutLine, RunOpts, run_cmd};

/// The `rpmbuild` topdir subdirectories created for every component.
const TOPDIR_SUBDIRS: [&str; 5] = ["BUILD", "SOURCES", "RPMS", "SRPMS", "SPECS"];

/// A component's RPM build result: the version built and the `rpmbuild` topdir
/// its artifacts landed under. Consumed by signing and S3 upload (C4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentBuild {
    pub version: String,
    pub rpms_path: Utf8PathBuf,
}

/// Build the RPMs for every component in `components` (`build_rpms` in
/// `rpmbuild.py`). Dependencies install sequentially first, then the per-
/// component builds run in parallel and fail-fast. Returns each component's
/// build result by name.
pub async fn build_rpms(
    rpms_path: &Utf8Path,
    el_version: u32,
    components_locs: &BTreeMap<String, CoreComponentLoc>,
    components: &BTreeMap<String, BuildComponentInfo>,
    ccache_path: Option<&Utf8Path>,
    skip_build: bool,
) -> Result<BTreeMap<String, ComponentBuild>, BuilderError> {
    // Every component must have a core definition.
    for name in components.keys() {
        if !components_locs.contains_key(name) {
            return Err(BuilderError::ComponentNotDefined(name.clone()));
        }
    }

    install_deps(components_locs, components).await?;

    // Resolve each buildable component's script + version up front. A component
    // whose directory is gone or that declares no rpm section is skipped with a
    // warning (`rpmbuild.py:215-223`); a declared-but-missing script is fatal.
    let mut to_build: Vec<ToBuild> = Vec::new();
    for (name, info) in components {
        let comp_loc = &components_locs[name];
        if !comp_loc.path.exists() {
            warn!(target: BUILDER, "component path for '{name}' not found at '{}'", comp_loc.path);
            continue;
        }
        let Some(rpm) = &comp_loc.comp.build.rpm else {
            warn!(target: BUILDER, "component '{name}' has no rpm build section; skipping");
            continue;
        };
        let build_script = comp_loc.path.join(&rpm.build);
        if !build_script.exists() {
            return Err(BuilderError::MissingScript {
                component: name.clone(),
                script: "build_rpms".to_string(),
                path: build_script,
            });
        }
        to_build.push(ToBuild {
            name: name.clone(),
            build_script,
            worktree: info.worktree_path.clone(),
            version: info.long_version.clone(),
        });
    }

    let mut set: JoinSet<Result<(String, ComponentBuild), BuilderError>> = JoinSet::new();
    for tb in to_build {
        let rpms_path = rpms_path.to_owned();
        let ccache = ccache_path.map(Utf8Path::to_owned);
        set.spawn(async move {
            let topdir = build_component(BuildJob {
                rpms_path: &rpms_path,
                el_version,
                comp_name: &tb.name,
                script_path: &tb.build_script,
                repo_path: &tb.worktree,
                version: &tb.version,
                ccache_path: ccache.as_deref(),
                skip_build,
            })
            .await?;
            Ok((
                tb.name,
                ComponentBuild {
                    version: tb.version,
                    rpms_path: topdir,
                },
            ))
        });
    }

    let mut builds = BTreeMap::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok((name, build))) => {
                builds.insert(name, build);
            }
            Ok(Err(err)) => {
                set.abort_all();
                return Err(err);
            }
            Err(join_err) => {
                set.abort_all();
                return Err(BuilderError::ComponentTaskFailed(join_err.to_string()));
            }
        }
    }
    Ok(builds)
}

/// One component queued for an RPM build.
struct ToBuild {
    name: String,
    build_script: Utf8PathBuf,
    worktree: Utf8PathBuf,
    version: String,
}

/// Install every component's build dependencies **sequentially** — the
/// `install_deps` scripts mutate shared dnf state, so they must not run in
/// parallel (`_install_deps` in `rpmbuild.py`).
async fn install_deps(
    components_locs: &BTreeMap<String, CoreComponentLoc>,
    components: &BTreeMap<String, BuildComponentInfo>,
) -> Result<(), BuilderError> {
    for (name, info) in components {
        let comp_loc = &components_locs[name];
        let script = comp_loc.path.join(&comp_loc.comp.build.deps);
        if !script.exists() {
            return Err(BuilderError::MissingScript {
                component: name.clone(),
                script: "install_deps".to_string(),
                path: script,
            });
        }
        let script = abs(&script)?;
        let worktree = abs(&info.worktree_path)?;

        info!(target: BUILDER, "install dependencies for component '{name}'");
        let out = run_cmd(
            &[
                CmdArg::from(script.as_str()),
                CmdArg::from(worktree.as_str()),
            ],
            RunOpts {
                cwd: Some(&worktree),
                out_cb: Some(&debug_log()),
                ..RunOpts::default()
            },
        )
        .await
        .map_err(|source| BuilderError::Command {
            context: format!("install_deps for '{name}'"),
            source,
        })?;
        if out.code != 0 {
            return Err(BuilderError::Step {
                context: format!("install_deps for '{name}'"),
                code: out.code,
                stderr: out.stderr,
            });
        }
    }
    Ok(())
}

/// The inputs to [`build_component`], grouped so the spawn site stays readable.
struct BuildJob<'a> {
    rpms_path: &'a Utf8Path,
    el_version: u32,
    comp_name: &'a str,
    script_path: &'a Utf8Path,
    repo_path: &'a Utf8Path,
    version: &'a str,
    ccache_path: Option<&'a Utf8Path>,
    skip_build: bool,
}

/// Build one component: lay out its topdir and run its `build_rpms` script there
/// (`_build_component` in `rpmbuild.py`). With `skip_build` the topdir is laid
/// out but the script does not run. Returns the topdir.
async fn build_component(job: BuildJob<'_>) -> Result<Utf8PathBuf, BuilderError> {
    let BuildJob {
        rpms_path,
        el_version,
        comp_name,
        script_path,
        repo_path,
        version,
        ccache_path,
        skip_build,
    } = job;

    let topdir = setup_rpm_topdir(rpms_path, comp_name, version)?;
    if skip_build {
        debug!(target: BUILDER, "skip_build set; laid out topdir for '{comp_name}', run nothing");
        return Ok(topdir);
    }

    let script = abs(script_path)?;
    let repo = abs(repo_path)?;
    // build_rpms <repo> <el_version> <topdir> <version>
    let args = [
        CmdArg::from(script.as_str()),
        CmdArg::from(repo.as_str()),
        CmdArg::from(el_version.to_string()),
        CmdArg::from(topdir.as_str()),
        CmdArg::from(version),
    ];

    // A configured ccache is passed through as CES_CCACHE_PATH (merged over the
    // inherited environment; the Rust runner has no Python venv to reset, unlike
    // Python's `reset_python_env`). It is made absolute (Python `resolve()`) so
    // the script — which runs with cwd=repo — does not resolve it against the
    // worktree.
    let extra_env: Vec<(String, String)> = match ccache_path {
        Some(ccache) => vec![(
            "CES_CCACHE_PATH".to_string(),
            abs_lenient(ccache).to_string(),
        )],
        None => vec![],
    };

    info!(target: BUILDER, "build component '{comp_name}' in '{repo}' using '{script}'");
    let out = run_cmd(
        &args,
        RunOpts {
            cwd: Some(&repo),
            extra_env: &extra_env,
            out_cb: Some(&debug_log()),
            ..RunOpts::default()
        },
    )
    .await
    .map_err(|source| BuilderError::Command {
        context: format!("build_rpms for '{comp_name}'"),
        source,
    })?;
    if out.code != 0 {
        return Err(BuilderError::Step {
            context: format!("build_rpms for '{comp_name}'"),
            code: out.code,
            stderr: out.stderr,
        });
    }
    Ok(topdir)
}

/// Lay out `<rpms>/<name>/<version>/` with the `rpmbuild` subdirectories and
/// return its absolute path (`_setup_rpm_topdir` in `rpmbuild.py`).
fn setup_rpm_topdir(
    rpms_path: &Utf8Path,
    component_name: &str,
    version: &str,
) -> Result<Utf8PathBuf, BuilderError> {
    let topdir = rpms_path.join(component_name).join(version);
    std::fs::create_dir_all(&topdir).map_err(|source| io_err(&topdir, source))?;
    for sub in TOPDIR_SUBDIRS {
        let dir = topdir.join(sub);
        std::fs::create_dir_all(&dir).map_err(|source| io_err(&dir, source))?;
    }
    abs(&topdir)
}

/// Make a path absolute without requiring it to exist (Python's `resolve()` on a
/// possibly-not-yet-created path, e.g. the ccache directory): a full
/// symlink-resolving canonicalize when it exists, else a filesystem-free
/// absolutize, else the path unchanged.
fn abs_lenient(path: &Utf8Path) -> Utf8PathBuf {
    path.canonicalize_utf8()
        .ok()
        .or_else(|| {
            std::path::absolute(path)
                .ok()
                .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        })
        .unwrap_or_else(|| path.to_owned())
}

/// Resolve a path to an absolute, symlink-free form (Python's `resolve()`).
/// Build scripts run with the worktree as cwd, so the script/repo/topdir paths
/// they receive must be absolute. Errors if the path does not exist (the script,
/// repo, and topdir always do by the time this is called).
fn abs(path: &Utf8Path) -> Result<Utf8PathBuf, BuilderError> {
    path.canonicalize_utf8()
        .map_err(|source| io_err(path, source))
}

/// A subprocess `out_cb` that streams each line to the builder's debug log
/// (which the host captures from the container). Shared across the builder
/// stages — `prepare_builder`/`install_cosign` (parent module) reuse it.
pub(super) fn debug_log() -> impl Fn(String) -> OutLine {
    |line: String| -> OutLine { Box::pin(async move { debug!(target: BUILDER, "{line}") }) }
}

fn io_err(path: &Utf8Path, source: std::io::Error) -> BuilderError {
    BuilderError::Io {
        context: path.to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::builder::test_support::retry_spawn;
    use crate::components::{
        CoreComponent, CoreComponentBuild, CoreComponentBuildRpm, CoreComponentContainers,
    };

    /// Write `contents` to `path` (creating parents) and mark it executable.
    fn write_script(path: &Utf8Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A component definition under `comp_dir` whose `deps`/`build`/`release-rpm`
    /// scripts are the given snippets (each made executable; an empty snippet is
    /// not written, so the script is "missing").
    struct CompFixture {
        loc: CoreComponentLoc,
    }

    fn component(
        comp_dir: &Utf8Path,
        name: &str,
        deps: Option<&str>,
        build: Option<&str>,
        has_rpm: bool,
    ) -> CompFixture {
        if let Some(deps) = deps {
            write_script(&comp_dir.join("deps.sh"), deps);
        }
        if let Some(build) = build {
            write_script(&comp_dir.join("build_rpms.sh"), build);
        }
        let rpm = has_rpm.then(|| CoreComponentBuildRpm {
            build: "build_rpms.sh".to_string(),
            release_rpm: "get_release_rpm.sh".to_string(),
        });
        CompFixture {
            loc: CoreComponentLoc {
                path: comp_dir.to_owned(),
                comp: CoreComponent {
                    name: name.to_string(),
                    repo: "https://github.com/ceph/ceph".to_string(),
                    build: CoreComponentBuild {
                        rpm,
                        get_version: "get_version.sh".to_string(),
                        deps: "deps.sh".to_string(),
                    },
                    containers: CoreComponentContainers {
                        path: "containers".into(),
                    },
                },
            },
        }
    }

    fn info(name: &str, worktree: &Utf8Path) -> BuildComponentInfo {
        std::fs::create_dir_all(worktree).unwrap();
        BuildComponentInfo {
            name: name.to_string(),
            repo_path: worktree.to_owned(),
            worktree_path: worktree.to_owned(),
            repo_url: "https://github.com/ceph/ceph".to_string(),
            base_ref: "main".to_string(),
            sha1: "deadbeef".to_string(),
            long_version: format!("{name}-1.2.3"),
        }
    }

    #[test]
    fn setup_rpm_topdir_lays_out_the_rpmbuild_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let rpms = Utf8Path::from_path(tmp.path()).unwrap();
        let topdir = setup_rpm_topdir(rpms, "ceph", "1.2.3").unwrap();
        assert!(topdir.as_str().ends_with("ceph/1.2.3"));
        for sub in TOPDIR_SUBDIRS {
            assert!(topdir.join(sub).is_dir(), "{sub} should exist");
        }
    }

    #[tokio::test]
    async fn install_deps_runs_each_component_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let order = base.join("order.txt");

        // Two components whose deps scripts append their name to a shared file.
        // BTreeMap iteration is sorted, so 'a' must run before 'b'.
        let mut locs = BTreeMap::new();
        let mut infos = BTreeMap::new();
        for name in ["a", "b"] {
            let comp_dir = base.join("components").join(name);
            let fix = component(
                &comp_dir,
                name,
                Some(&format!("#!/bin/sh\necho {name} >> {order}\n")),
                None,
                false,
            );
            locs.insert(name.to_string(), fix.loc);
            infos.insert(name.to_string(), info(name, &base.join("wt").join(name)));
        }

        retry_spawn(|| install_deps(&locs, &infos)).await;
        assert_eq!(std::fs::read_to_string(&order).unwrap(), "a\nb\n");
    }

    #[tokio::test]
    async fn install_deps_errors_when_a_script_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("components").join("ceph");
        // No deps script written.
        let fix = component(&comp_dir, "ceph", None, None, false);
        let locs = BTreeMap::from([("ceph".to_string(), fix.loc)]);
        let infos = BTreeMap::from([("ceph".to_string(), info("ceph", &base.join("wt")))]);

        let err = install_deps(&locs, &infos).await.unwrap_err();
        assert!(matches!(err, BuilderError::MissingScript { .. }), "{err}");
    }

    #[tokio::test]
    async fn build_rpms_lays_out_topdirs_and_runs_the_build_script() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("components").join("ceph");
        // build_rpms <repo> <el> <topdir> <version> → drop an artifact in RPMS.
        let fix = component(
            &comp_dir,
            "ceph",
            Some("#!/bin/sh\nexit 0\n"),
            Some("#!/bin/sh\ntouch \"$3/RPMS/ceph.rpm\"\n"),
            true,
        );
        let locs = BTreeMap::from([("ceph".to_string(), fix.loc)]);
        let infos = BTreeMap::from([("ceph".to_string(), info("ceph", &base.join("wt")))]);

        let rpms = base.join("rpms");
        let builds = retry_spawn(|| build_rpms(&rpms, 9, &locs, &infos, None, false)).await;

        let build = &builds["ceph"];
        assert_eq!(build.version, "ceph-1.2.3");
        assert!(build.rpms_path.as_str().ends_with("ceph/ceph-1.2.3"));
        assert!(
            build.rpms_path.join("RPMS").join("ceph.rpm").is_file(),
            "the build script's artifact landed in RPMS"
        );
    }

    #[tokio::test]
    async fn build_rpms_skip_build_lays_out_topdir_but_runs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("components").join("ceph");
        let fix = component(
            &comp_dir,
            "ceph",
            Some("#!/bin/sh\nexit 0\n"),
            Some("#!/bin/sh\ntouch \"$3/RPMS/ceph.rpm\"\n"),
            true,
        );
        let locs = BTreeMap::from([("ceph".to_string(), fix.loc)]);
        let infos = BTreeMap::from([("ceph".to_string(), info("ceph", &base.join("wt")))]);

        let rpms = base.join("rpms");
        let builds = retry_spawn(|| build_rpms(&rpms, 9, &locs, &infos, None, true)).await;

        let build = &builds["ceph"];
        // The topdir exists, but the build script never ran.
        assert!(build.rpms_path.join("RPMS").is_dir());
        assert!(!build.rpms_path.join("RPMS").join("ceph.rpm").exists());
    }

    #[tokio::test]
    async fn build_rpms_passes_the_ccache_path_through() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("components").join("ceph");
        let fix = component(
            &comp_dir,
            "ceph",
            Some("#!/bin/sh\nexit 0\n"),
            Some("#!/bin/sh\nprintf '%s' \"$CES_CCACHE_PATH\" > \"$3/ccache.txt\"\n"),
            true,
        );
        let locs = BTreeMap::from([("ceph".to_string(), fix.loc)]);
        let infos = BTreeMap::from([("ceph".to_string(), info("ceph", &base.join("wt")))]);

        let rpms = base.join("rpms");
        let ccache = base.join("ccache");
        std::fs::create_dir_all(&ccache).unwrap();
        let builds =
            retry_spawn(|| build_rpms(&rpms, 9, &locs, &infos, Some(&ccache), false)).await;

        let recorded =
            std::fs::read_to_string(builds["ceph"].rpms_path.join("ccache.txt")).unwrap();
        assert!(
            recorded.ends_with("ccache"),
            "CES_CCACHE_PATH was {recorded}"
        );
    }

    #[tokio::test]
    async fn build_rpms_skips_a_component_with_no_rpm_section() {
        let tmp = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap();
        let comp_dir = base.join("components").join("ceph");
        // has_rpm = false → no rpm build section.
        let fix = component(&comp_dir, "ceph", Some("#!/bin/sh\nexit 0\n"), None, false);
        let locs = BTreeMap::from([("ceph".to_string(), fix.loc)]);
        let infos = BTreeMap::from([("ceph".to_string(), info("ceph", &base.join("wt")))]);

        let rpms = base.join("rpms");
        let builds = retry_spawn(|| build_rpms(&rpms, 9, &locs, &infos, None, false)).await;
        assert!(
            builds.is_empty(),
            "component with no rpm section is skipped"
        );
    }
}
