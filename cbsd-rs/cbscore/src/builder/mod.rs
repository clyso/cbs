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
//! C2b lands stage 1 — [`Builder::prepare_builder`] installs the build
//! toolchain — wired as the first step of [`Builder::run`], which still emits a
//! `skipped` report afterwards. The remaining stages — rpmbuild → sign → upload
//! → container image — and the decision flow land in C3–C6.

use camino::Utf8PathBuf;
use cbscore_types::{BuildArtifactReport, Config, VersionDescriptor};
use tracing::debug;

use crate::types::tracing_targets;
use crate::utils::redact::CmdArg;
use crate::utils::subprocess::{CommandError, OutLine, RunOpts, run_cmd};

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
    /// C2b: install the toolchain, then emit a `skipped` report. The decision
    /// flow (skopeo short-circuit, release reuse) and the remaining stages land
    /// in C3–C6.
    pub async fn run(&self) -> Result<BuildArtifactReport, BuilderError> {
        debug!(
            target: tracing_targets::BUILDER,
            "building {} (skip_build={}, force={}, tls_verify={})",
            self.desc.version, self.opts.skip_build, self.opts.force, self.opts.tls_verify
        );
        self.prepare_builder().await?;
        let report = self.skipped_report();
        self.write_report(&report).await?;
        Ok(report)
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

    /// The keystone's `skipped` report (C3–C6 replace this with the real build).
    fn skipped_report(&self) -> BuildArtifactReport {
        BuildArtifactReport {
            report_version: 1,
            version: self.desc.version.clone(),
            skipped: true,
            container_image: None,
            release_descriptor: None,
            components: vec![],
        }
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
    async fn write_report_lands_the_skipped_report_on_the_scratch_mount() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = Utf8Path::from_path(dir.path()).unwrap();
        let config = config_with_scratch(scratch);
        let desc = descriptor();
        let builder = Builder::new(&config, &desc, opts());

        let report = builder.skipped_report();
        assert!(report.skipped);
        assert_eq!(report.version, "20.2.1");
        builder.write_report(&report).await.unwrap();

        let written = tokio::fs::read_to_string(scratch.join(BUILD_REPORT_FILE))
            .await
            .unwrap();
        let parsed: BuildArtifactReport = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed, report);
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
}
