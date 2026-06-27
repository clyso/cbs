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

//! The in-container builder pipeline (design 007). This runs **inside** the
//! builder container (the host runner that spawns it is design 009); it writes
//! `build-report.json` to the scratch mount, which the host reads back.
//!
//! C2b landed stage 1 — [`Builder::prepare_builder`] installs the build
//! toolchain. C3 adds stages 2–3: [`Builder::run`] now prepares the components
//! (clone/checkout/patch) and builds their RPMs, and reports the built
//! components. The remaining stages — sign → upload → container image — and the
//! decision flow (skopeo short-circuit, release reuse) land in C4–C6, where the
//! report's `container_image`/`release_descriptor` and the components'
//! `rpms_s3_path` fill in.

use std::collections::BTreeMap;
use std::sync::Arc;

use camino::Utf8PathBuf;
use cbscore_types::{BuildArtifactReport, ComponentReport, Config, VersionDescriptor};
use tracing::{debug, info};

use crate::builder::prepare::{BuildComponentInfo, prepare_components};
use crate::builder::rpmbuild::build_rpms;
use crate::components::load_components;
use crate::config::get_secrets;
use crate::types::tracing_targets;
use crate::utils::git::GitError;
use crate::utils::redact::CmdArg;
use crate::utils::secrets::{SecretsError, SecretsMgr};
use crate::utils::subprocess::{CommandError, OutLine, RunOpts, run_cmd};

pub mod prepare;
pub mod rpmbuild;

#[cfg(test)]
mod test_support;

/// The filename the report is written under, on the scratch mount.
pub const BUILD_REPORT_FILE: &str = "build-report.json";

/// The pinned cosign release RPM installed in the builder (design 007;
/// `prepare.py`). The version pin is carried over from Python verbatim.
const COSIGN_RPM_URL: &str =
    "https://github.com/sigstore/cosign/releases/download/v2.4.3/cosign-2.4.3-1.x86_64.rpm";

/// Build behaviour flags threaded from the CLI (design 010). The full pipeline
/// (C2b–C6) consumes these; the keystone only records them.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub skip_build: bool,
    pub force: bool,
    pub tls_verify: bool,
}

/// An error from the in-container build.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// A toolchain command could not be spawned or timed out.
    #[error("error running '{context}'")]
    Command {
        context: String,
        #[source]
        source: CommandError,
    },
    /// A toolchain command exited non-zero.
    #[error("'{context}' exited {code}: {stderr}")]
    Step {
        context: String,
        code: i32,
        stderr: String,
    },
    #[error("error writing build report to '{path}'")]
    WriteReport {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error serialising build report")]
    Serialize(#[source] serde_json::Error),
    /// No `cbs.component.yaml` defines a component named in the version.
    #[error("no core component definition for '{0}'")]
    ComponentNotDefined(String),
    /// A git operation failed while preparing a component.
    #[error("git error preparing component '{component}'")]
    Git {
        component: String,
        #[source]
        source: GitError,
    },
    /// Resolving a component's git URL against the configured secrets failed.
    #[error("error resolving the git url for component '{component}'")]
    Secrets {
        component: String,
        #[source]
        source: SecretsError,
    },
    /// A required component script is missing on disk.
    #[error("missing '{script}' script for component '{component}' (expected at '{path}')")]
    MissingScript {
        component: String,
        script: String,
        path: Utf8PathBuf,
    },
    /// A filesystem operation around component preparation failed.
    #[error("filesystem error ({context})")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// A per-component preparation task could not be joined (panicked/aborted).
    #[error("component preparation task failed: {0}")]
    ComponentTaskFailed(String),
    /// The build's secrets could not be assembled from the config.
    #[error("error setting up secrets")]
    SecretsSetup(#[source] crate::config::ConfigError),
    /// The component definitions could not be loaded.
    #[error("error loading component definitions")]
    LoadComponents(#[source] crate::components::ComponentError),
    /// No component definitions were found in the configured paths.
    #[error("no components found in the configured component paths")]
    NoComponents,
}

/// One `prepare_builder` toolchain step: the command argv and a context label
/// for its error. A free function (not a method) so the sequence is unit-testable
/// for parity with `prepare.py` without running anything.
fn prepare_steps() -> Vec<(Vec<&'static str>, &'static str)> {
    vec![
        (vec!["dnf", "update", "-y"], "dnf update"),
        (
            vec!["dnf", "install", "-y", "epel-release"],
            "installing epel-release",
        ),
        (
            vec!["dnf", "config-manager", "--enable", "crb"],
            "enabling the crb repo",
        ),
        (vec!["dnf", "update", "-y"], "dnf update after crb"),
        (
            vec![
                "dnf",
                "install",
                "-y",
                "git",
                "wget",
                "rpm-build",
                "rpmdevtools",
                "gcc-c++",
                "createrepo",
                "rpm-sign",
                "pinentry",
                "s3cmd",
                "jq",
                "ccache",
                "buildah",
                "skopeo",
            ],
            "installing the build toolchain",
        ),
    ]
}

/// The in-container build orchestrator (design 007).
pub struct Builder<'a> {
    config: &'a Config,
    desc: &'a VersionDescriptor,
    opts: BuildOptions,
}

impl<'a> Builder<'a> {
    pub fn new(config: &'a Config, desc: &'a VersionDescriptor, opts: BuildOptions) -> Self {
        Self { config, desc, opts }
    }

    /// Run the in-container build and write its report to the scratch mount.
    ///
    /// C3: install the toolchain, prepare the components (clone/checkout/patch),
    /// build their RPMs, and report the built components. Signing, S3 upload, the
    /// container image, and the skopeo/release decision flow land in C4–C6.
    pub async fn run(&self) -> Result<BuildArtifactReport, BuilderError> {
        debug!(
            target: tracing_targets::BUILDER,
            "building {} (skip_build={}, force={}, tls_verify={})",
            self.desc.version, self.opts.skip_build, self.opts.force, self.opts.tls_verify
        );
        self.prepare_builder().await?;
        let report = self.build().await?;
        self.write_report(&report).await?;
        Ok(report)
    }

    /// Prepare the components and build their RPMs, then report them (stages 2–3
    /// of `Builder._build_release`). The prepared worktrees are cleaned up on
    /// both the success and the failure path, mirroring Python's
    /// `prepare_components` context manager.
    async fn build(&self) -> Result<BuildArtifactReport, BuilderError> {
        let secrets = Arc::new(SecretsMgr::new(
            get_secrets(self.config)
                .await
                .map_err(BuilderError::SecretsSetup)?,
        ));
        let components_loc = Arc::new(
            load_components(&self.config.paths.components)
                .await
                .map_err(BuilderError::LoadComponents)?,
        );
        if components_loc.is_empty() {
            return Err(BuilderError::NoComponents);
        }

        let prepared = prepare_components(
            Arc::clone(&secrets),
            &self.config.paths.scratch,
            Arc::clone(&components_loc),
            &self.desc.components,
            &self.desc.version,
        )
        .await?;

        let outcome = self.build_rpms_for(&components_loc, &prepared).await;
        prepared.cleanup().await;
        outcome.map(|()| self.build_report(prepared.infos()))
    }

    /// Build the RPMs for the prepared components (the artifacts land under
    /// `<scratch>/rpms/<name>/<version>/`). The resulting build map feeds signing
    /// and S3 upload in C4; here it only proves the RPMs were produced.
    async fn build_rpms_for(
        &self,
        components_loc: &BTreeMap<String, crate::components::CoreComponentLoc>,
        prepared: &prepare::PreparedComponents,
    ) -> Result<(), BuilderError> {
        let rpms_path = self.config.paths.scratch.join("rpms");
        tokio::fs::create_dir_all(&rpms_path)
            .await
            .map_err(|source| BuilderError::Io {
                context: format!("creating '{rpms_path}'"),
                source,
            })?;
        let builds = build_rpms(
            &rpms_path,
            self.desc.el_version,
            components_loc,
            prepared.infos(),
            self.config.paths.ccache.as_deref(),
            self.opts.skip_build,
        )
        .await?;
        info!(target: tracing_targets::BUILDER, "built RPMs for {} component(s)", builds.len());
        Ok(())
    }

    /// Build the report from the prepared components. `skipped` is false and the
    /// components carry their version/sha1/repo; the image, release descriptor,
    /// and per-component S3 paths stay unset until C4–C6.
    fn build_report(&self, infos: &BTreeMap<String, BuildComponentInfo>) -> BuildArtifactReport {
        let components = infos
            .values()
            .map(|info| ComponentReport {
                name: info.name.clone(),
                version: info.long_version.clone(),
                sha1: info.sha1.clone(),
                repo_url: info.repo_url.clone(),
                rpms_s3_path: None,
            })
            .collect();
        BuildArtifactReport {
            report_version: 1,
            version: self.desc.version.clone(),
            skipped: false,
            container_image: None,
            release_descriptor: None,
            components,
        }
    }

    /// Stage 1 (design 007 / `prepare.py`): update dnf, enable EPEL and CRB, and
    /// install the build toolchain, then cosign from its pinned release RPM. Each
    /// step streams its output through the subprocess `out_cb` (here, to the
    /// builder's debug log, which the host captures from the container).
    pub async fn prepare_builder(&self) -> Result<(), BuilderError> {
        let log_cb = |line: String| -> OutLine {
            Box::pin(async move { debug!(target: tracing_targets::BUILDER, "{line}") })
        };
        for (args, context) in prepare_steps() {
            let cmd: Vec<CmdArg> = args.iter().map(|s| CmdArg::from(*s)).collect();
            let out = run_cmd(
                &cmd,
                RunOpts {
                    out_cb: Some(&log_cb),
                    ..RunOpts::default()
                },
            )
            .await
            .map_err(|source| BuilderError::Command {
                context: context.to_string(),
                source,
            })?;
            if out.code != 0 {
                return Err(BuilderError::Step {
                    context: context.to_string(),
                    code: out.code,
                    stderr: out.stderr,
                });
            }
        }
        self.install_cosign().await
    }

    /// Install cosign from the pinned release RPM, tolerating the
    /// already-installed case (`rpm -Uvh` exits 2 with "already installed"),
    /// exactly as `prepare.py` does.
    async fn install_cosign(&self) -> Result<(), BuilderError> {
        let cmd = [
            CmdArg::from("rpm"),
            CmdArg::from("-Uvh"),
            CmdArg::from(COSIGN_RPM_URL),
        ];
        let out =
            run_cmd(&cmd, RunOpts::default())
                .await
                .map_err(|source| BuilderError::Command {
                    context: "installing cosign".to_string(),
                    source,
                })?;
        if out.code == 2 && out.stderr.contains("already installed") {
            debug!(target: tracing_targets::BUILDER, "cosign already installed; skipping");
            return Ok(());
        }
        if out.code != 0 {
            return Err(BuilderError::Step {
                context: "installing cosign".to_string(),
                code: out.code,
                stderr: out.stderr,
            });
        }
        Ok(())
    }

    async fn write_report(&self, report: &BuildArtifactReport) -> Result<(), BuilderError> {
        let path = self.config.paths.scratch.join(BUILD_REPORT_FILE);
        let json = serde_json::to_string_pretty(report).map_err(BuilderError::Serialize)?;
        tokio::fs::write(&path, json)
            .await
            .map_err(|source| BuilderError::WriteReport { path, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8Path;

    use crate::types::VersionComponent;

    fn config_with_scratch(scratch: &Utf8Path) -> Config {
        Config {
            schema_version: 1,
            paths: cbscore_types::PathsConfig {
                components: vec!["components".into()],
                scratch: scratch.to_owned(),
                scratch_containers: "/var/lib/containers".into(),
                ccache: None,
            },
            storage: None,
            signing: None,
            logging: None,
            secrets: vec![],
            vault: None,
        }
    }

    fn descriptor() -> VersionDescriptor {
        VersionDescriptor {
            schema_version: 1,
            version: "20.2.1".to_string(),
            title: "Release version 20.2.1".to_string(),
            signed_off_by: cbscore_types::VersionSignedOffBy {
                user: "Jane".to_string(),
                email: "jane@example.com".to_string(),
            },
            image: cbscore_types::VersionImage {
                registry: "harbor.clyso.com".to_string(),
                name: "ces/ceph/ceph".to_string(),
                tag: "20.2.1".to_string(),
            },
            components: vec![],
            distro: "rockylinux:9".to_string(),
            el_version: 9,
        }
    }

    fn opts() -> BuildOptions {
        BuildOptions {
            skip_build: false,
            force: false,
            tls_verify: true,
        }
    }

    // The report path is tested directly rather than through `run`, which now
    // installs the toolchain via dnf and so only runs inside the container.
    #[tokio::test]
    async fn write_report_lands_the_report_on_the_scratch_mount() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = Utf8Path::from_path(dir.path()).unwrap();
        let config = config_with_scratch(scratch);
        let desc = descriptor();
        let builder = Builder::new(&config, &desc, opts());

        let report = builder.build_report(&BTreeMap::new());
        assert!(!report.skipped);
        assert_eq!(report.version, "20.2.1");
        builder.write_report(&report).await.unwrap();

        let written = tokio::fs::read_to_string(scratch.join(BUILD_REPORT_FILE))
            .await
            .unwrap();
        let parsed: BuildArtifactReport = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed, report);
    }

    #[test]
    fn build_report_lists_components_without_upload_or_image() {
        let config = config_with_scratch(Utf8Path::new("/scratch"));
        let desc = descriptor();
        let builder = Builder::new(&config, &desc, opts());

        let infos = BTreeMap::from([(
            "ceph".to_string(),
            BuildComponentInfo {
                name: "ceph".to_string(),
                repo_path: "/r".into(),
                worktree_path: "/w".into(),
                repo_url: "https://github.com/ceph/ceph".to_string(),
                base_ref: "v20.2.1".to_string(),
                sha1: "5a0b003".to_string(),
                long_version: "20.2.1-42.g5a0b003".to_string(),
            },
        )]);

        let report = builder.build_report(&infos);
        assert!(!report.skipped);
        assert!(report.container_image.is_none());
        assert!(report.release_descriptor.is_none());
        assert_eq!(report.components.len(), 1);
        let comp = &report.components[0];
        assert_eq!(comp.name, "ceph");
        assert_eq!(comp.version, "20.2.1-42.g5a0b003");
        assert_eq!(comp.sha1, "5a0b003");
        // Not uploaded yet — S3 path fills in at C4.
        assert!(comp.rpms_s3_path.is_none());
    }

    #[test]
    fn prepare_steps_match_the_python_toolchain_sequence() {
        let steps = prepare_steps();
        let argvs: Vec<&Vec<&str>> = steps.iter().map(|(a, _)| a).collect();
        assert_eq!(*argvs[0], vec!["dnf", "update", "-y"]);
        assert_eq!(*argvs[1], vec!["dnf", "install", "-y", "epel-release"]);
        assert_eq!(*argvs[2], vec!["dnf", "config-manager", "--enable", "crb"]);
        assert_eq!(*argvs[3], vec!["dnf", "update", "-y"]);
        // The toolchain install carries the full package set (prepare.py:82-100).
        let toolchain = argvs[4];
        assert_eq!(&toolchain[0..3], &["dnf", "install", "-y"]);
        for pkg in [
            "git",
            "rpm-build",
            "rpmdevtools",
            "createrepo",
            "rpm-sign",
            "buildah",
            "skopeo",
            "ccache",
        ] {
            assert!(toolchain.contains(&pkg), "missing package {pkg}");
        }
        assert_eq!(steps.len(), 5);
    }

    #[test]
    fn cosign_rpm_is_pinned() {
        assert!(COSIGN_RPM_URL.ends_with("v2.4.3/cosign-2.4.3-1.x86_64.rpm"));
    }

    /// Run the real toolchain sequence in `rockylinux:9` and confirm the key
    /// tools (and cosign) end up installed. Ignored by default: it needs podman
    /// and network and installs ~hundreds of MB. Run with
    /// `cargo test -p cbscore --lib -- --ignored prepare_toolchain`.
    #[tokio::test]
    #[ignore = "requires podman and network; installs the full toolchain (slow)"]
    async fn prepare_toolchain_installs_in_rockylinux() {
        use crate::utils::podman::{RunArgs, podman_run};

        let mut script = String::from("set -eux\n");
        for (args, _) in prepare_steps() {
            script.push_str(&args.join(" "));
            script.push('\n');
        }
        script.push_str(&format!("rpm -Uvh {COSIGN_RPM_URL}\n"));
        script.push_str("command -v buildah\ncommand -v skopeo\ncommand -v cosign\n");

        let args = RunArgs {
            args: vec!["sh".to_string(), "-c".to_string(), script],
            timeout: Some(std::time::Duration::from_secs(540)),
            ..Default::default()
        };
        let out = podman_run("rockylinux:9", &args)
            .await
            .expect("podman run should spawn");
        assert_eq!(
            out.code, 0,
            "toolchain install failed\nstdout:\n{}\nstderr:\n{}",
            out.stdout, out.stderr
        );
    }

    // ----- Builder::build orchestration (container-independent) ----------
    //
    // `build` does not call `prepare_builder`, so it runs no dnf/podman: it
    // clones a local `file://` repo and runs the component's own shell scripts.
    // That makes the full stage-2/3 orchestration — and the "always clean up the
    // worktrees, on success and on failure" guarantee — testable on the host.

    /// A minimal secrets file so `get_secrets` succeeds; its single entry never
    /// matches the `file://` component repo, so the clone uses the URL verbatim.
    const SECRETS_YAML: &str = "git:\n  github.com:\n    creds: plain\n    type: https\n    username: u\n    password: p\n";

    /// A component manifest with deps + rpm build scripts (kebab-case keys, as
    /// `load_components` parses them).
    const COMPONENT_YAML: &str = "name: ceph\nrepo: https://github.com/ceph/ceph\nbuild:\n  rpm:\n    build: build_rpms.sh\n    release-rpm: get_release_rpm.sh\n  get-version: get_version.sh\n  deps: deps.sh\ncontainers:\n  path: containers\n";

    fn write_exec(path: &Utf8Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    async fn git(dir: &Utf8Path, args: &[&str]) {
        let status = tokio::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .await
            .expect("git available");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Init a source repo with a README on a `testref` branch.
    async fn init_source(dir: &Utf8Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]).await;
        git(dir, &["config", "user.name", "Test"]).await;
        git(dir, &["config", "user.email", "test@example.com"]).await;
        std::fs::write(dir.join("README"), "hello\n").unwrap();
        git(dir, &["add", "."]).await;
        git(dir, &["commit", "-q", "-m", "init"]).await;
        git(dir, &["branch", "testref"]).await;
    }

    /// An on-disk fixture for `Builder::build`: a `file://` source repo, a
    /// component definition (manifest + get_version/deps/build scripts), a
    /// secrets file, and a scratch dir, wired into an owned `Config` +
    /// `VersionDescriptor`. The build-script body is the caller's, so a test can
    /// drive either the success or the failure path.
    struct BuildFixture {
        _tmp: tempfile::TempDir,
        config: Config,
        desc: VersionDescriptor,
    }

    async fn build_fixture(build_script: &str) -> BuildFixture {
        // A dot-free prefix so the `file://` URL validates (the git-URL grammar's
        // path class excludes '.').
        let tmp = tempfile::Builder::new()
            .prefix("cbsbuild")
            .tempdir()
            .unwrap();
        let base = Utf8Path::from_path(tmp.path()).unwrap().to_owned();

        let source = base.join("source");
        init_source(&source).await;
        let repo_url = format!("file://{source}");

        let comp_dir = base.join("components").join("ceph");
        write_exec(
            &comp_dir.join("get_version.sh"),
            "#!/bin/sh\necho 1.2.3-build\n",
        );
        write_exec(&comp_dir.join("deps.sh"), "#!/bin/sh\nexit 0\n");
        write_exec(&comp_dir.join("build_rpms.sh"), build_script);
        std::fs::write(comp_dir.join("cbs.component.yaml"), COMPONENT_YAML).unwrap();

        let secrets_file = base.join("secrets.yaml");
        std::fs::write(&secrets_file, SECRETS_YAML).unwrap();

        let mut config = config_with_scratch(&base.join("scratch"));
        config.paths.components = vec![base.join("components")];
        config.secrets = vec![secrets_file];

        let mut desc = descriptor();
        desc.components = vec![VersionComponent {
            name: "ceph".to_string(),
            repo: repo_url,
            git_ref: "testref".to_string(),
        }];

        BuildFixture {
            _tmp: tmp,
            config,
            desc,
        }
    }

    /// The component worktrees must be gone once `build` returns — on both the
    /// success and the failure path. `git worktree remove` drops the worktree
    /// leaf but leaves its (now empty) `<name>/` parent, exactly as Python does,
    /// so assert no checked-out *files* remain rather than an empty tree.
    fn assert_worktrees_cleaned(scratch: &Utf8Path) {
        let worktrees = scratch.join("git").join("worktrees");
        let left = count_files(worktrees.as_std_path());
        assert_eq!(left, 0, "checked-out files left under '{worktrees}'");
    }

    /// Count regular files anywhere under `dir` (a missing dir counts as zero).
    fn count_files(dir: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| {
                let path = e.path();
                if path.is_dir() { count_files(&path) } else { 1 }
            })
            .sum()
    }

    /// Retry `op` while it yields a transient spawn error. The exec-spawning
    /// builder tests can hit a multithreaded write-then-exec `ETXTBSY` race on a
    /// freshly written script; that surfaces as `BuilderError::Command` (a spawn
    /// failure), distinct from a `Step` (the script ran and exited non-zero), so
    /// retrying on `Command` never masks a real build failure.
    async fn retry_transient<F, Fut, T>(mut op: F) -> Result<T, BuilderError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, BuilderError>>,
    {
        for _ in 0..25 {
            match op().await {
                Err(BuilderError::Command { .. }) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                other => return other,
            }
        }
        panic!("operation kept hitting transient spawn errors");
    }

    #[tokio::test]
    async fn build_prepares_compiles_and_cleans_up_on_success() {
        retry_transient(|| async {
            let fx = build_fixture("#!/bin/sh\ntouch \"$3/RPMS/ceph.rpm\"\n").await;
            let builder = Builder::new(&fx.config, &fx.desc, opts());
            let report = builder.build().await?;

            assert!(!report.skipped);
            assert_eq!(report.version, "20.2.1");
            assert_eq!(report.components.len(), 1);
            assert_eq!(report.components[0].name, "ceph");
            assert_eq!(report.components[0].version, "1.2.3-build");
            assert!(report.components[0].rpms_s3_path.is_none());
            assert_worktrees_cleaned(&fx.config.paths.scratch);
            Ok(())
        })
        .await
        .expect("build should succeed");
    }

    #[tokio::test]
    async fn build_cleans_up_worktrees_when_a_component_build_fails() {
        retry_transient(|| async {
            let fx = build_fixture("#!/bin/sh\nexit 1\n").await;
            let builder = Builder::new(&fx.config, &fx.desc, opts());
            let err = builder
                .build()
                .await
                .expect_err("build must fail when build_rpms exits non-zero");

            // A spawn race (Command) is retried; a real build failure is a Step.
            if matches!(err, BuilderError::Command { .. }) {
                return Err(err);
            }
            assert!(matches!(err, BuilderError::Step { .. }), "{err}");
            assert_worktrees_cleaned(&fx.config.paths.scratch);
            Ok(())
        })
        .await
        .expect("build failure path asserted");
    }
}
