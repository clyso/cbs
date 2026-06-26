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

//! The `build` (host) and hidden `runner build` (in-container) handlers
//! (design 010). `build` is thin glue: load the config, fold the `--sign-with-*`
//! overrides, pre-check `--log-file`, then call `cbscore::runner::run`. It does
//! **no** secrets marshalling — that lives entirely in the runner (009), fixing
//! Python's dead `cmd_build` plaintext-secrets write.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use cbscore::builder::{BuildOptions, Builder};
use cbscore::runner::{CancellationToken, RunOpts, run};
use cbscore::versions::read_descriptor;

use crate::bool_parser::debug_enabled;
use crate::cli::{BuildArgs, RunnerBuildArgs};

/// The env var pointing at the musl `cbsbuild` artifact to mount into the
/// builder container. The worker image sets it; the CLI falls back to the
/// running binary for local use (design 001 B1 / 009 — an explicit path, never a
/// uv/venv bootstrap).
const RUNNER_BIN_ENV: &str = "CBS_RUNNER_BIN";

/// Convert the global `--config` path to UTF-8, erroring clearly otherwise.
fn config_path(config: &Path) -> Result<&Utf8Path, ExitCode> {
    Utf8Path::from_path(config).ok_or_else(|| {
        eprintln!("error: config path is not valid UTF-8");
        ExitCode::FAILURE
    })
}

/// Handle `cbsbuild build`. Returns `0` on success, `1` on a runtime failure
/// (clap handles usage errors with exit `2`).
pub async fn build(config: &Path, debug: bool, args: &BuildArgs) -> ExitCode {
    let config_path = match config_path(config) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let mut cfg = match cbscore::config::load_config(config_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error loading config: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Fold the signing overrides into the config (creating the block if absent).
    if args.sign_with_gpg_id.is_some() || args.sign_with_transit.is_some() {
        let signing = cfg.signing.get_or_insert(cbscore::types::SigningConfig {
            gpg: None,
            transit: None,
        });
        if let Some(gpg) = &args.sign_with_gpg_id {
            signing.gpg = Some(gpg.clone());
        }
        if let Some(transit) = &args.sign_with_transit {
            signing.transit = Some(transit.clone());
        }
    }

    // Refuse to clobber an existing log file (Python's pre-check).
    if let Some(log_file) = &args.log_file
        && tokio::fs::try_exists(log_file).await.unwrap_or(false)
    {
        eprintln!("error: log file '{log_file}' already exists");
        return ExitCode::FAILURE;
    }

    let opts = RunOpts {
        run_name: None,
        replace_if_exists: false,
        timeout: Duration::from_secs_f64(args.timeout),
        skip_build: args.skip_build,
        force: args.force,
        tls_verify: args.tls_verify,
        debug: debug_enabled(debug),
        log_file: args.log_file.clone(),
        cancel: CancellationToken::new(),
        cbsbuild_bin: resolve_runner_bin(),
    };

    match run(&args.descriptor, &cfg, opts).await {
        Ok(_report) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("build failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Handle the hidden `cbsbuild runner build` (in-container). The HOME hook runs
/// first (design 010): set `HOME=/runner` only when it is unset/empty or `/`, so
/// an image that ships `HOME=/root` keeps it.
pub async fn runner_build(config: &Path, args: &RunnerBuildArgs) -> ExitCode {
    apply_home_hook();

    let config_path = match config_path(config) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let cfg = match cbscore::config::load_config(config_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error loading config: {e}");
            return ExitCode::FAILURE;
        }
    };
    let desc = match read_descriptor(&args.desc).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error reading descriptor: {e}");
            return ExitCode::FAILURE;
        }
    };

    let opts = BuildOptions {
        skip_build: args.skip_build,
        force: args.force,
        tls_verify: args.tls_verify,
    };
    match Builder::new(&cfg, &desc, opts).run().await {
        Ok(_report) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the `cbsbuild` artifact to mount: the `CBS_RUNNER_BIN` path when set,
/// otherwise the running binary (which must be the musl build for cross-distro
/// use). The runner validates it exists and is executable.
fn resolve_runner_bin() -> Utf8PathBuf {
    if let Ok(path) = std::env::var(RUNNER_BIN_ENV) {
        return Utf8PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("cbsbuild"))
}

/// Replicate `cbscore-entrypoint.sh`: set `HOME=/runner` iff unset/empty or `/`.
fn apply_home_hook() {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() || home == "/" {
        // SAFETY: this is the first action of the in-container `runner build`
        // entry, before any concurrent environment access in this short-lived
        // PID-1 process; no other thread reads HOME at this point.
        unsafe { std::env::set_var("HOME", "/runner") };
    }
}
