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

//! The `podman` shell-tool wrapper (design 003). Source: `utils/podman.py`.
//!
//! [`podman_run`] launches the builder container; [`podman_stop`] tears it
//! down. A non-zero container exit is **not** an error here — it is returned as
//! a [`CmdOutput`] for the caller (the runner, design 009) to interpret, exactly
//! as Python returns `(rc, stdout, stderr)`. Only a timeout/cancel is fatal:
//! podman is given its own `--timeout` *and* the same wall-clock deadline on the
//! await (the two race, as in Python), and on elapse the wrapper reads the
//! cidfile and stops the container by id before erroring.
//!
//! Live per-line output streaming (the `out_cb` of design 003) lands with its
//! first consumer — the runner's log sink (009) — in the next commit; here the
//! output is collected and returned.

use std::time::Duration;

use camino::Utf8Path;
use tracing::warn;

use crate::types::tracing_targets;
use crate::utils::redact::CmdArg;
use crate::utils::subprocess::{CmdOutput, CommandError, RunOpts, run_cmd};

/// Grace period handed to `podman stop` before it escalates to SIGKILL, matching
/// Python's `podman_stop` default of one second.
const DEFAULT_STOP_GRACE: Duration = Duration::from_secs(1);

/// An error from a podman invocation.
#[derive(Debug, thiserror::Error)]
pub enum PodmanError {
    /// The run hit its deadline (or was cancelled); the container was stopped.
    #[error("podman command timed out or was cancelled")]
    Timeout,
    /// The podman process could not be spawned or read.
    #[error("podman command failed to run")]
    Command(#[from] CommandError),
    /// The cidfile (where podman records the container id) could not be created.
    #[error("failed to create podman cidfile")]
    Cidfile(#[source] std::io::Error),
}

/// Parameters for [`podman_run`], mirroring Python's keyword arguments. Ordered
/// `Vec`s (not maps) keep the emitted argv deterministic. The runner (009) fills
/// the mount table, env, devices, and entrypoint.
#[derive(Debug, Default, Clone)]
pub struct RunArgs {
    /// Arguments passed to the container after the image (the in-container argv).
    pub args: Vec<String>,
    /// `--env K=V`, in order.
    pub env: Vec<(String, String)>,
    /// `--volume SRC:DST`, in order.
    pub volumes: Vec<(String, String)>,
    /// `--device SRC:DST`, in order.
    pub devices: Vec<(String, String)>,
    /// `--entrypoint`, overriding the image's.
    pub entrypoint: Option<String>,
    /// `--name` for the container.
    pub name: Option<String>,
    /// `--userns keep-id`.
    pub use_user_ns: bool,
    /// podman's own `--timeout` and the await deadline (both, as in Python).
    pub timeout: Option<Duration>,
    /// `--network host`.
    pub use_host_network: bool,
    /// `--security-opt seccomp=unconfined`.
    pub unconfined: bool,
    /// `--replace` an existing container of the same name.
    pub replace_if_exists: bool,
}

/// Build the `podman run` argv (everything after `podman`) for a given image
/// and parameters, in the exact order Python emits (`podman.py:58-106`). Pure,
/// so the runner's mount/flag assembly (009) is testable without a container.
fn build_run_argv(image: &str, args: &RunArgs, cidfile: &str) -> Vec<String> {
    let mut cmd: Vec<String> = vec![
        "run".to_string(),
        "--security-opt".to_string(),
        "label=disable".to_string(),
        "--cidfile".to_string(),
        cidfile.to_string(),
        "--attach".to_string(),
        "stdout".to_string(),
        "--attach".to_string(),
        "stderr".to_string(),
    ];
    if let Some(name) = &args.name {
        cmd.push("--name".to_string());
        cmd.push(name.clone());
    }
    if args.use_user_ns {
        cmd.push("--userns".to_string());
        cmd.push("keep-id".to_string());
    }
    if let Some(timeout) = args.timeout {
        cmd.push("--timeout".to_string());
        cmd.push(timeout.as_secs().to_string());
    }
    if args.unconfined {
        cmd.push("--security-opt".to_string());
        cmd.push("seccomp=unconfined".to_string());
    }
    if args.replace_if_exists {
        cmd.push("--replace".to_string());
    }
    for (key, value) in &args.env {
        cmd.push("--env".to_string());
        cmd.push(format!("{key}={value}"));
    }
    for (src, dst) in &args.volumes {
        cmd.push("--volume".to_string());
        cmd.push(format!("{src}:{dst}"));
    }
    for (src, dst) in &args.devices {
        cmd.push("--device".to_string());
        cmd.push(format!("{src}:{dst}"));
    }
    if args.use_host_network {
        cmd.push("--network".to_string());
        cmd.push("host".to_string());
    }
    if let Some(entrypoint) = &args.entrypoint {
        cmd.push("--entrypoint".to_string());
        cmd.push(entrypoint.clone());
    }
    cmd.push(image.to_string());
    cmd.extend(args.args.iter().cloned());
    cmd
}

/// The `podman stop` argv (everything after `podman`): stop `name`, or `--all`
/// when none is given.
fn build_stop_argv(name: Option<&str>, timeout: Duration) -> Vec<String> {
    vec![
        "stop".to_string(),
        "--time".to_string(),
        timeout.as_secs().to_string(),
        name.unwrap_or("--all").to_string(),
    ]
}

/// Run a podman container to completion, collecting its output. A non-zero exit
/// is returned in the [`CmdOutput`] (logged, not raised). On a timeout the
/// container is stopped via its cidfile and [`PodmanError::Timeout`] is returned.
pub async fn podman_run(image: &str, args: &RunArgs) -> Result<CmdOutput, PodmanError> {
    // podman records the container id here; we read it back only to stop the
    // container on a timeout. A fresh temp dir gives a path podman can create
    // (podman refuses a cidfile that already exists).
    let cid_dir = tempfile::Builder::new()
        .prefix("cbscore-")
        .tempdir()
        .map_err(PodmanError::Cidfile)?;
    let cidfile = Utf8Path::from_path(cid_dir.path())
        .ok_or_else(|| {
            PodmanError::Cidfile(std::io::Error::other("temp dir path is not valid UTF-8"))
        })?
        .join("container.cid");

    let argv: Vec<CmdArg> = std::iter::once(CmdArg::from("podman"))
        .chain(
            build_run_argv(image, args, cidfile.as_str())
                .into_iter()
                .map(CmdArg::from),
        )
        .collect();

    let opts = RunOpts {
        timeout: args.timeout,
        ..RunOpts::default()
    };
    match run_cmd(&argv, opts).await {
        Ok(output) => {
            if output.code != 0 {
                warn!(
                    target: tracing_targets::PODMAN,
                    "podman run exited {}: {}", output.code, output.stderr
                );
            }
            Ok(output)
        }
        Err(CommandError::Timeout) => {
            warn!(
                target: tracing_targets::PODMAN,
                "podman run timed out or was cancelled; stopping the container"
            );
            if let Ok(cid) = tokio::fs::read_to_string(&cidfile).await {
                let cid = cid.trim();
                if !cid.is_empty() {
                    let _ = podman_stop(Some(cid), DEFAULT_STOP_GRACE).await;
                }
            }
            Err(PodmanError::Timeout)
        }
        Err(other) => Err(PodmanError::Command(other)),
    }
}

/// Stop a container by `name` (or `--all` when `None`). A non-zero exit is
/// logged, not raised, matching Python's `podman_stop`.
pub async fn podman_stop(name: Option<&str>, timeout: Duration) -> Result<(), PodmanError> {
    let argv: Vec<CmdArg> = std::iter::once(CmdArg::from("podman"))
        .chain(build_stop_argv(name, timeout).into_iter().map(CmdArg::from))
        .collect();
    let output = run_cmd(&argv, RunOpts::default()).await?;
    if output.code != 0 {
        warn!(
            target: tracing_targets::PODMAN,
            "error stopping container: {}", output.stderr
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn run_argv_assembles_every_flag_in_python_order() {
        let args = RunArgs {
            args: strs(&["--config", "/c.yaml", "runner", "build"]),
            env: vec![("CBS_DEBUG".to_string(), "1".to_string())],
            volumes: vec![("/host".to_string(), "/runner".to_string())],
            devices: vec![("/dev/fuse".to_string(), "/dev/fuse".to_string())],
            entrypoint: Some("/runner/cbsbuild".to_string()),
            name: Some("ces_abc".to_string()),
            use_user_ns: true,
            timeout: Some(Duration::from_secs(120)),
            use_host_network: true,
            unconfined: true,
            replace_if_exists: true,
        };
        let argv = build_run_argv("rockylinux:9", &args, "/tmp/x.cid");
        assert_eq!(
            argv,
            strs(&[
                "run",
                "--security-opt",
                "label=disable",
                "--cidfile",
                "/tmp/x.cid",
                "--attach",
                "stdout",
                "--attach",
                "stderr",
                "--name",
                "ces_abc",
                "--userns",
                "keep-id",
                "--timeout",
                "120",
                "--security-opt",
                "seccomp=unconfined",
                "--replace",
                "--env",
                "CBS_DEBUG=1",
                "--volume",
                "/host:/runner",
                "--device",
                "/dev/fuse:/dev/fuse",
                "--network",
                "host",
                "--entrypoint",
                "/runner/cbsbuild",
                "rockylinux:9",
                "--config",
                "/c.yaml",
                "runner",
                "build",
            ])
        );
    }

    #[test]
    fn run_argv_minimal_is_prelude_then_image() {
        let argv = build_run_argv("alpine", &RunArgs::default(), "/x.cid");
        assert_eq!(
            argv,
            strs(&[
                "run",
                "--security-opt",
                "label=disable",
                "--cidfile",
                "/x.cid",
                "--attach",
                "stdout",
                "--attach",
                "stderr",
                "alpine",
            ])
        );
    }

    #[test]
    fn stop_argv_targets_name_or_all() {
        assert_eq!(
            build_stop_argv(Some("ces_abc"), Duration::from_secs(1)),
            strs(&["stop", "--time", "1", "ces_abc"])
        );
        assert_eq!(
            build_stop_argv(None, Duration::from_secs(5)),
            strs(&["stop", "--time", "5", "--all"])
        );
    }

    /// End-to-end proof that the wrapper spins a real container and collects its
    /// output. Ignored by default: it needs podman on PATH and network to pull
    /// the image. Run with `cargo test -p cbscore -- --ignored`.
    #[tokio::test]
    #[ignore = "requires podman and network to pull an image"]
    async fn spins_a_real_container_and_collects_output() {
        let name = format!("cbscore-podman-test-{}", std::process::id());
        let args = RunArgs {
            args: strs(&["echo", "cbscore-ok"]),
            name: Some(name.clone()),
            replace_if_exists: true,
            ..Default::default()
        };
        let result = podman_run("alpine", &args).await;
        // Best-effort cleanup regardless of the assertion outcome below.
        let _ = tokio::process::Command::new("podman")
            .args(["rm", "-f", &name])
            .output()
            .await;
        let output = result.expect("podman run should succeed");
        assert_eq!(output.code, 0, "stderr: {}", output.stderr);
        assert!(
            output.stdout.contains("cbscore-ok"),
            "stdout: {}",
            output.stdout
        );
    }
}
