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

//! The host-side two-phase runner (design 009). It prepares the build's inputs
//! on the host, spawns the builder container with everything mounted, and reads
//! the build report back across the scratch mount.
//!
//! Source: `cbscore/runner.py` (and the entrypoint it replaces). The Rust port
//! mounts the compiled musl `cbsbuild` binary at `/runner/cbsbuild` and runs it
//! as PID 1 directly — there is no source-tree mount and no uv/venv bootstrap
//! (design 001 B1). Every temp input (the aggregated components, the **plaintext
//! secrets file**, and the rewritten config) is held in one staging dir that is
//! removed on every return path, fixing Python's plaintext-secrets leak (009).
//!
//! Live per-line output streaming (the worker's callback / the CLI's stdout
//! sink, design 009) lands with the worker's real build output in a later
//! commit; here the container output is collected and written to `log_file`
//! when one is given.

use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use cbscore_types::{BuildArtifactReport, Config};
use tracing::warn;

// Re-exported so the runner's callers (the CLI, the worker) construct a token
// without taking a direct `tokio-util` dependency.
pub use tokio_util::sync::CancellationToken;

use crate::builder::BUILD_REPORT_FILE;
use crate::config::{
    ConfigError, SecretsError, get_secrets, get_vault_config, store_config, store_secrets,
    store_vault,
};
use crate::types::tracing_targets;
use crate::utils::podman::{PodmanError, RunArgs, podman_run, podman_stop};
use crate::versions::{ReadError, read_descriptor};

// The fixed in-container paths everything is mounted at (design 009 mount table).
const C_CBSBUILD: &str = "/runner/cbsbuild";
const C_CONFIG: &str = "/runner/cbs-build.config.yaml";
const C_SECRETS: &str = "/runner/cbs-build.secrets.yaml";
const C_VAULT: &str = "/runner/cbs-build.vault.yaml";
const C_SCRATCH: &str = "/runner/scratch";
const C_SCRATCH_CONTAINERS: &str = "/var/lib/containers";
const C_COMPONENTS: &str = "/runner/components";
const C_CCACHE: &str = "/runner/ccache";
const FUSE_DEVICE: &str = "/dev/fuse";

/// Grace period for stopping the container on cancellation.
const STOP_GRACE: Duration = Duration::from_secs(1);

/// Options for a single [`run`] (design 009).
pub struct RunOpts {
    /// The container name; a fresh `ces_…` name is generated when `None`.
    pub run_name: Option<String>,
    /// Pass `--replace` to podman.
    pub replace_if_exists: bool,
    /// The build timeout (podman's `--timeout` and the await deadline).
    pub timeout: Duration,
    /// Forwarded to the in-container build (`--skip-build`).
    pub skip_build: bool,
    /// Forwarded to the in-container build (`--force`).
    pub force: bool,
    /// Forwarded as `--tls-verify=<bool>` to the in-container build.
    pub tls_verify: bool,
    /// The host's effective debug state, forwarded as `CBS_DEBUG=<1|0>`.
    pub debug: bool,
    /// Where to write the collected container output; stdout sink lands later.
    pub log_file: Option<Utf8PathBuf>,
    /// Fires to cancel the build; the runner then stops the container by name.
    pub cancel: CancellationToken,
    /// The musl `cbsbuild` artifact mounted as PID 1 — an explicit path, never
    /// "self" (design 001 B1).
    pub cbsbuild_bin: Utf8PathBuf,
}

/// An error from the host runner (design 009).
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// The descriptor file is absent or invalid.
    #[error(transparent)]
    Descriptor(#[from] ReadError),
    /// The `cbsbuild` artifact to mount is missing or not executable.
    #[error("cbsbuild artifact '{path}' does not exist or is not executable")]
    Artifact { path: Utf8PathBuf },
    /// The container ran to completion but exited non-zero. Carries the partial
    /// report read before the rc check (design 009 / invariant 5).
    #[error("the build exited non-zero")]
    NonZeroExit {
        report: Option<BuildArtifactReport>,
        stderr: String,
    },
    /// podman failed or the build timed out; no report.
    #[error(transparent)]
    Podman(#[from] PodmanError),
    /// The caller's cancellation token fired; the container was stopped by name.
    #[error("the build was cancelled")]
    Cancelled,
    /// A config/secrets marshalling failure while preparing the temp inputs.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A secrets marshalling failure.
    #[error(transparent)]
    Secrets(#[from] SecretsError),
    /// A filesystem failure preparing or recovering the build's inputs/outputs.
    #[error("runner IO error ({context})")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

/// Run a build: prepare inputs, spawn the builder container, recover the report.
///
/// Returns the report (`None` if the container wrote none) on success. A
/// non-zero container exit returns [`RunnerError::NonZeroExit`] carrying the
/// partial report (read before the rc check); a timeout/podman failure or a
/// cancellation carries no report (design 009).
pub async fn run(
    desc_path: &Utf8Path,
    config: &Config,
    opts: RunOpts,
) -> Result<Option<BuildArtifactReport>, RunnerError> {
    // 1. Validate inputs: the descriptor parses, and the artifact is runnable.
    let desc = read_descriptor(desc_path).await?;
    validate_artifact(&opts.cbsbuild_bin).await?;

    // 2. One staging dir holds every temp input and is removed on every return
    //    path (RAII), so the plaintext secrets file never outlives the build.
    let staging = tempfile::Builder::new()
        .prefix("cbscore-runner-")
        .tempdir()
        .map_err(|source| RunnerError::Io {
            context: "creating the staging directory".to_string(),
            source,
        })?;
    let staging_path = Utf8Path::from_path(staging.path()).ok_or_else(|| RunnerError::Io {
        context: "staging path is not valid UTF-8".to_string(),
        source: std::io::Error::other("non-UTF-8 staging path"),
    })?;

    // 2a. Aggregate every component subdirectory into one /runner/components.
    let components_dir = staging_path.join("components");
    aggregate_components(&config.paths.components, &components_dir).await?;

    // 3. Marshal the merged secrets into the staging dir.
    let secrets = get_secrets(config).await?;
    let secrets_file = staging_path.join("cbs-build.secrets.yaml");
    store_secrets(&secrets, &secrets_file).await?;

    // 3a. Marshal the vault config too, when one is configured.
    let vault_file = match get_vault_config(config).await? {
        Some(vault) => {
            let path = staging_path.join("cbs-build.vault.yaml");
            store_vault(&vault, &path).await?;
            Some(path)
        }
        None => None,
    };

    // 4. Rewrite the config for in-container paths and marshal it.
    let container_config = rewrite_config(config);
    let config_file = staging_path.join("cbs-build.config.yaml");
    store_config(&container_config, &config_file).await?;

    // 5. Assemble the podman invocation and spawn the builder container. Host
    //    mount sources are made absolute (podman rejects relative bind sources,
    //    and the report read joins on the scratch path), matching Python's
    //    `.resolve()` of every volume source. The staging paths are already
    //    absolute.
    let ctr_name = opts.run_name.clone().unwrap_or_else(gen_run_name);
    let desc_name = desc_path.file_name().unwrap_or("descriptor.json");
    let abs_cbsbuild = absolutize(&opts.cbsbuild_bin);
    let abs_desc = absolutize(desc_path);
    let abs_scratch = absolutize(&config.paths.scratch);
    let abs_scratch_containers = absolutize(&config.paths.scratch_containers);
    let abs_ccache = config.paths.ccache.as_deref().map(absolutize);
    let mounts = MountPaths {
        cbsbuild_bin: &abs_cbsbuild,
        desc_host: &abs_desc,
        desc_container_name: desc_name,
        config_file: &config_file,
        secrets_file: &secrets_file,
        vault_file: vault_file.as_deref(),
        components_dir: &components_dir,
        host_scratch: &abs_scratch,
        host_scratch_containers: &abs_scratch_containers,
        host_ccache: abs_ccache.as_deref(),
    };
    let run_args = build_run_args(&mounts, &opts, &ctr_name);
    let image = desc.distro.clone();

    // 6. Spawn, racing the run against the caller's cancellation token. On
    //    cancel we stop the container by its known name (dropping the podman
    //    future runs no async cleanup), and the staging dir is removed on return.
    let result = tokio::select! {
        result = podman_run(&image, &run_args) => result,
        _ = opts.cancel.cancelled() => {
            warn!(target: tracing_targets::RUNNER, "build cancelled; stopping {ctr_name}");
            let _ = podman_stop(Some(&ctr_name), STOP_GRACE).await;
            return Err(RunnerError::Cancelled);
        }
    };

    // 7. Recover the report from the host scratch mount **before** the rc check,
    //    then surface the outcome (design 009 / invariant 5).
    match result {
        Ok(output) => {
            let report = read_report(&abs_scratch).await;
            write_log(opts.log_file.as_deref(), &output).await;
            if output.code != 0 {
                return Err(RunnerError::NonZeroExit {
                    report,
                    stderr: output.stderr,
                });
            }
            Ok(report)
        }
        Err(e) => Err(RunnerError::Podman(e)),
    }
}

/// The host mount sources resolved for one build.
struct MountPaths<'a> {
    cbsbuild_bin: &'a Utf8Path,
    desc_host: &'a Utf8Path,
    desc_container_name: &'a str,
    config_file: &'a Utf8Path,
    secrets_file: &'a Utf8Path,
    vault_file: Option<&'a Utf8Path>,
    components_dir: &'a Utf8Path,
    host_scratch: &'a Utf8Path,
    host_scratch_containers: &'a Utf8Path,
    host_ccache: Option<&'a Utf8Path>,
}

/// Assemble the [`RunArgs`] for a build: the mount table, env, devices, flags,
/// and the in-container `runner build` argv (design 009). Pure, so the assembly
/// is testable without spawning a container.
fn build_run_args(mounts: &MountPaths, opts: &RunOpts, ctr_name: &str) -> RunArgs {
    let desc_container = format!("/runner/{}", mounts.desc_container_name);

    let mut volumes: Vec<(String, String)> = vec![
        (mounts.cbsbuild_bin.to_string(), C_CBSBUILD.to_string()),
        (mounts.desc_host.to_string(), desc_container.clone()),
        (mounts.config_file.to_string(), C_CONFIG.to_string()),
        (mounts.secrets_file.to_string(), C_SECRETS.to_string()),
    ];
    if let Some(vault) = mounts.vault_file {
        volumes.push((vault.to_string(), C_VAULT.to_string()));
    }
    volumes.push((mounts.host_scratch.to_string(), C_SCRATCH.to_string()));
    // The containers-storage mount carries the `:Z` SELinux relabel (design 009).
    volumes.push((
        mounts.host_scratch_containers.to_string(),
        format!("{C_SCRATCH_CONTAINERS}:Z"),
    ));
    volumes.push((mounts.components_dir.to_string(), C_COMPONENTS.to_string()));
    if let Some(ccache) = mounts.host_ccache {
        volumes.push((ccache.to_string(), C_CCACHE.to_string()));
    }

    let mut args = vec![
        "--config".to_string(),
        C_CONFIG.to_string(),
        "runner".to_string(),
        "build".to_string(),
        "--desc".to_string(),
        desc_container,
        format!("--tls-verify={}", opts.tls_verify),
    ];
    if opts.skip_build {
        args.push("--skip-build".to_string());
    }
    if opts.force {
        args.push("--force".to_string());
    }

    RunArgs {
        args,
        // `CBS_DEBUG` is forwarded; HOME is deliberately not host-set (design 009
        // — the conditional HOME lives in the in-container `runner build` entry).
        env: vec![(
            "CBS_DEBUG".to_string(),
            if opts.debug { "1" } else { "0" }.to_string(),
        )],
        volumes,
        // Python passes `/dev/fuse:rw`; podman defaults to `rwm` without an
        // explicit mode, but keep the mode for fidelity (design 009 / runner.py).
        devices: vec![(FUSE_DEVICE.to_string(), format!("{FUSE_DEVICE}:rw"))],
        entrypoint: Some(C_CBSBUILD.to_string()),
        name: Some(ctr_name.to_string()),
        use_user_ns: false,
        timeout: Some(opts.timeout),
        use_host_network: true,
        unconfined: true,
        replace_if_exists: opts.replace_if_exists,
    }
}

/// Clone `config` with the in-container `/runner/...` paths (design 009 step 4).
/// `logging` is cleared to `None` rather than pointed at an unmounted path: the
/// in-container binary then logs to stderr, which the host streams.
fn rewrite_config(config: &Config) -> Config {
    let mut c = config.clone();
    c.paths.scratch = C_SCRATCH.into();
    c.paths.scratch_containers = C_SCRATCH_CONTAINERS.into();
    c.paths.components = vec![C_COMPONENTS.into()];
    if c.paths.ccache.is_some() {
        c.paths.ccache = Some(C_CCACHE.into());
    }
    c.secrets = vec![C_SECRETS.into()];
    if c.vault.is_some() {
        c.vault = Some(C_VAULT.into());
    }
    c.logging = None;
    c
}

/// Copy every immediate subdirectory of each `component_paths` entry into one
/// `dest` directory (Python's `_setup_components_dir`), so the container sees a
/// single `/runner/components`.
async fn aggregate_components(
    component_paths: &[Utf8PathBuf],
    dest: &Utf8Path,
) -> Result<(), RunnerError> {
    let io = |context: &str, source: std::io::Error| RunnerError::Io {
        context: context.to_string(),
        source,
    };
    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| io("creating the components staging dir", e))?;
    for base in component_paths {
        let mut entries = tokio::fs::read_dir(base)
            .await
            .map_err(|e| io("reading a components path", e))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| io("listing a components path", e))?
        {
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let Ok(src) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
            let name = src.file_name().unwrap_or_default();
            copy_tree(&src, &dest.join(name))
                .await
                .map_err(|e| io("copying a component directory", e))?;
        }
    }
    Ok(())
}

/// Recursively copy the directory tree at `src` to `dst`. Iterative (an explicit
/// stack) to avoid boxing an async recursion.
async fn copy_tree(src: &Utf8Path, dst: &Utf8Path) -> std::io::Result<()> {
    let mut stack = vec![(src.to_owned(), dst.to_owned())];
    while let Some((s, d)) = stack.pop() {
        tokio::fs::create_dir_all(&d).await?;
        let mut entries = tokio::fs::read_dir(&s).await?;
        while let Some(entry) = entries.next_entry().await? {
            let Ok(child) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
            let name = child.file_name().unwrap_or_default().to_string();
            let target = d.join(&name);
            if entry.file_type().await?.is_dir() {
                stack.push((child, target));
            } else {
                tokio::fs::copy(&child, &target).await?;
            }
        }
    }
    Ok(())
}

/// Make a host path absolute. podman rejects relative bind-mount sources, and
/// the report read joins on the scratch path, so the runner resolves every host
/// source — mirroring Python's `.resolve()`. `std::path::absolute` does not
/// require the path to exist (e.g. a not-yet-created ccache), unlike
/// `canonicalize`.
fn absolutize(path: &Utf8Path) -> Utf8PathBuf {
    std::path::absolute(path)
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| path.to_owned())
}

/// Read the build report from the host scratch mount and unlink it (design 009 /
/// invariant 5). Returns `None` when no readable report was written.
async fn read_report(scratch: &Utf8Path) -> Option<BuildArtifactReport> {
    let path = scratch.join(BUILD_REPORT_FILE);
    let raw = tokio::fs::read_to_string(&path).await.ok()?;
    let report = match serde_json::from_str(&raw) {
        Ok(report) => Some(report),
        Err(e) => {
            warn!(target: tracing_targets::RUNNER, "build report at '{path}' is unparseable: {e}");
            None
        }
    };
    let _ = tokio::fs::remove_file(&path).await;
    report
}

/// Write the collected container output to `log_file` when one is given. The
/// live stdout/callback sink (design 009) lands with the worker's build output.
async fn write_log(log_file: Option<&Utf8Path>, output: &crate::utils::subprocess::CmdOutput) {
    let Some(path) = log_file else { return };
    let combined = format!("{}{}", output.stdout, output.stderr);
    if let Err(e) = tokio::fs::write(path, combined).await {
        warn!(target: tracing_targets::RUNNER, "could not write log to '{path}': {e}");
    }
}

/// Check the `cbsbuild` artifact to mount exists and is executable (replacing
/// Python's entrypoint-script validation; design 009 step 1).
async fn validate_artifact(path: &Utf8Path) -> Result<(), RunnerError> {
    use std::os::unix::fs::PermissionsExt;
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.is_file() && meta.permissions().mode() & 0o111 != 0 => Ok(()),
        _ => Err(RunnerError::Artifact {
            path: path.to_owned(),
        }),
    }
}

/// A collision-avoiding container name: `ces_` plus ten random lowercase ASCII
/// letters (Python `random.choices(string.ascii_lowercase, k=10)`; the value is
/// not security-sensitive, only unique).
fn gen_run_name() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let suffix: String = (0..10)
        .map(|_| (b'a' + rng.random_range(0..26u8)) as char)
        .collect();
    format!("ces_{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            schema_version: 1,
            paths: cbscore_types::PathsConfig {
                components: vec!["/host/components".into()],
                scratch: "/host/scratch".into(),
                scratch_containers: "/host/containers".into(),
                ccache: None,
            },
            storage: None,
            signing: None,
            logging: Some(cbscore_types::LoggingConfig {
                log_file: "/host/log".into(),
            }),
            secrets: vec!["/host/secrets.yaml".into()],
            vault: None,
        }
    }

    fn opts(name: &str) -> RunOpts {
        RunOpts {
            run_name: Some(name.to_string()),
            replace_if_exists: true,
            timeout: Duration::from_secs(14400),
            skip_build: false,
            force: false,
            tls_verify: true,
            debug: false,
            log_file: None,
            cancel: CancellationToken::new(),
            cbsbuild_bin: "/usr/local/bin/cbsbuild".into(),
        }
    }

    #[test]
    fn gen_run_name_is_prefixed_and_lowercase() {
        let name = gen_run_name();
        assert!(name.starts_with("ces_"));
        let suffix = name.strip_prefix("ces_").unwrap();
        assert_eq!(suffix.len(), 10);
        assert!(suffix.bytes().all(|b| b.is_ascii_lowercase()));
        // Two draws differ (collision-avoiding).
        assert_ne!(gen_run_name(), gen_run_name());
    }

    #[test]
    fn rewrite_config_uses_in_container_paths_and_clears_logging() {
        let mut cfg = base_config();
        cfg.paths.ccache = Some("/host/ccache".into());
        cfg.vault = Some("/host/vault.yaml".into());
        let c = rewrite_config(&cfg);
        assert_eq!(c.paths.scratch, C_SCRATCH);
        assert_eq!(c.paths.scratch_containers, C_SCRATCH_CONTAINERS);
        assert_eq!(c.paths.components, vec![Utf8PathBuf::from(C_COMPONENTS)]);
        assert_eq!(c.paths.ccache.as_deref(), Some(Utf8Path::new(C_CCACHE)));
        assert_eq!(c.secrets, vec![Utf8PathBuf::from(C_SECRETS)]);
        assert_eq!(c.vault.as_deref(), Some(Utf8Path::new(C_VAULT)));
        assert!(c.logging.is_none(), "in-container logging must be cleared");
    }

    #[test]
    fn rewrite_config_leaves_unset_optionals_unset() {
        let c = rewrite_config(&base_config());
        assert!(c.paths.ccache.is_none());
        assert!(c.vault.is_none());
    }

    #[test]
    fn run_args_build_the_full_mount_and_argv() {
        let mounts = MountPaths {
            cbsbuild_bin: Utf8Path::new("/host/cbsbuild"),
            desc_host: Utf8Path::new("/host/_versions/dev/20.2.1.json"),
            desc_container_name: "20.2.1.json",
            config_file: Utf8Path::new("/staging/cbs-build.config.yaml"),
            secrets_file: Utf8Path::new("/staging/cbs-build.secrets.yaml"),
            vault_file: None,
            components_dir: Utf8Path::new("/staging/components"),
            host_scratch: Utf8Path::new("/host/scratch"),
            host_scratch_containers: Utf8Path::new("/host/containers"),
            host_ccache: None,
        };
        let mut o = opts("ces_test");
        o.debug = true;
        o.skip_build = true;
        let run_args = build_run_args(&mounts, &o, "ces_test");

        // Mounts in order; no vault/ccache when unset.
        assert_eq!(
            run_args.volumes,
            vec![
                ("/host/cbsbuild".to_string(), C_CBSBUILD.to_string()),
                (
                    "/host/_versions/dev/20.2.1.json".to_string(),
                    "/runner/20.2.1.json".to_string()
                ),
                (
                    "/staging/cbs-build.config.yaml".to_string(),
                    C_CONFIG.to_string()
                ),
                (
                    "/staging/cbs-build.secrets.yaml".to_string(),
                    C_SECRETS.to_string()
                ),
                ("/host/scratch".to_string(), C_SCRATCH.to_string()),
                (
                    "/host/containers".to_string(),
                    "/var/lib/containers:Z".to_string()
                ),
                ("/staging/components".to_string(), C_COMPONENTS.to_string()),
            ]
        );
        // The in-container argv the host emits (design 009/010).
        assert_eq!(
            run_args.args,
            vec![
                "--config",
                C_CONFIG,
                "runner",
                "build",
                "--desc",
                "/runner/20.2.1.json",
                "--tls-verify=true",
                "--skip-build",
            ]
        );
        assert_eq!(
            run_args.env,
            vec![("CBS_DEBUG".to_string(), "1".to_string())]
        );
        assert_eq!(
            run_args.devices,
            vec![(FUSE_DEVICE.to_string(), format!("{FUSE_DEVICE}:rw"))]
        );
        assert_eq!(run_args.entrypoint.as_deref(), Some(C_CBSBUILD));
        assert!(run_args.use_host_network && run_args.unconfined && !run_args.use_user_ns);
    }

    #[test]
    fn run_args_add_vault_and_ccache_mounts_and_force_when_set() {
        let mounts = MountPaths {
            cbsbuild_bin: Utf8Path::new("/b"),
            desc_host: Utf8Path::new("/d.json"),
            desc_container_name: "d.json",
            config_file: Utf8Path::new("/c.yaml"),
            secrets_file: Utf8Path::new("/s.yaml"),
            vault_file: Some(Utf8Path::new("/v.yaml")),
            components_dir: Utf8Path::new("/comp"),
            host_scratch: Utf8Path::new("/scratch"),
            host_scratch_containers: Utf8Path::new("/ctr"),
            host_ccache: Some(Utf8Path::new("/ccache")),
        };
        let mut o = opts("ces_x");
        o.force = true;
        let run_args = build_run_args(&mounts, &o, "ces_x");
        assert!(
            run_args
                .volumes
                .contains(&("/v.yaml".to_string(), C_VAULT.to_string()))
        );
        assert!(
            run_args
                .volumes
                .contains(&("/ccache".to_string(), C_CCACHE.to_string()))
        );
        assert!(run_args.args.contains(&"--force".to_string()));
        assert!(!run_args.args.contains(&"--skip-build".to_string()));
    }

    #[tokio::test]
    async fn read_report_returns_and_unlinks() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = Utf8Path::from_path(dir.path()).unwrap();
        let report = BuildArtifactReport {
            report_version: 1,
            version: "20.2.1".to_string(),
            skipped: true,
            container_image: None,
            release_descriptor: None,
            components: vec![],
        };
        tokio::fs::write(
            scratch.join(BUILD_REPORT_FILE),
            serde_json::to_string(&report).unwrap(),
        )
        .await
        .unwrap();

        let read = read_report(scratch).await;
        assert_eq!(read, Some(report));
        // Unlinked after the read.
        assert!(
            !tokio::fs::try_exists(scratch.join(BUILD_REPORT_FILE))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn read_report_absent_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            read_report(Utf8Path::from_path(dir.path()).unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn validate_artifact_requires_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(dir.path()).unwrap();

        let plain = base.join("not-exec");
        tokio::fs::write(&plain, "x").await.unwrap();
        assert!(matches!(
            validate_artifact(&plain).await,
            Err(RunnerError::Artifact { .. })
        ));

        let exec = base.join("cbsbuild");
        tokio::fs::write(&exec, "#!/bin/sh\n").await.unwrap();
        let mut perm = tokio::fs::metadata(&exec).await.unwrap().permissions();
        perm.set_mode(0o755);
        tokio::fs::set_permissions(&exec, perm).await.unwrap();
        assert!(validate_artifact(&exec).await.is_ok());
    }

    #[tokio::test]
    async fn aggregate_components_copies_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let base = Utf8Path::from_path(dir.path()).unwrap();
        let src = base.join("components");
        tokio::fs::create_dir_all(src.join("ceph/nested"))
            .await
            .unwrap();
        tokio::fs::write(src.join("ceph/cbs.component.yaml"), "name: ceph")
            .await
            .unwrap();
        tokio::fs::write(src.join("ceph/nested/patch"), "p")
            .await
            .unwrap();

        let dest = base.join("agg");
        aggregate_components(&[src], &dest).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(dest.join("ceph/cbs.component.yaml"))
                .await
                .unwrap(),
            "name: ceph"
        );
        assert!(
            tokio::fs::try_exists(dest.join("ceph/nested/patch"))
                .await
                .unwrap()
        );
    }
}
