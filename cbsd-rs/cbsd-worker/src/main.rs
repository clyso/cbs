// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

mod build;
mod config;
mod signal;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

use crate::config::WorkerConfig;
use crate::signal::{ShutdownState, install_signal_handler};
use crate::ws::connection::reconnect_loop;

/// Extended version: cargo version + git SHA from production build.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("CBS_BUILD_META"),
);

/// CBS build worker — connects to the CBS server via WebSocket and executes
/// build jobs.
#[derive(Parser)]
#[command(name = "cbsd-worker", version = VERSION, about)]
struct Cli {
    /// Path to the worker configuration YAML file.
    #[arg(short, long, default_value = "worker.yaml")]
    config: PathBuf,
}

/// Set up tracing with optional file and console layers.
///
/// Console output is enabled when `CBSD_DEV` is set. File output is
/// enabled when `log_file` is configured. The returned guard must be
/// held for the process lifetime to flush the non-blocking file writer.
fn setup_tracing(
    level: &str,
    log_file: Option<&std::path::Path>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let is_dev = std::env::var("CBSD_DEV")
        .map(|v| !v.is_empty())
        .unwrap_or(false);

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let console_layer = if is_dev {
        Some(fmt::layer().with_ansi(true))
    } else {
        None
    };

    let (file_layer, guard) = if let Some(path) = log_file {
        let dir = path.parent().unwrap_or_else(|| {
            panic!(
                "config error: logging.log-file has no parent directory: '{}'",
                path.display()
            )
        });
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or_else(|| {
            panic!(
                "config error: logging.log-file has no filename component: '{}'",
                path.display()
            )
        });
        let appender = tracing_appender::rolling::never(dir, filename);
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let layer = fmt::layer().with_ansi(false).with_writer(writer);
        (Some(layer), Some(guard))
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    guard
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Load and resolve configuration (token or legacy fields).
    let raw_config = match WorkerConfig::load(&cli.config) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    // Set up tracing — hold the guard for the process lifetime.
    // Uses raw_config.logging before resolve() consumes it,
    // so that token warnings from resolve() are captured.
    let _guard = setup_tracing(
        &raw_config.logging.level,
        raw_config.logging.log_file.as_deref(),
    );

    let config = match raw_config.resolve() {
        Ok(c) => c,
        Err(err) => {
            // Use eprintln! — the tracing subscriber may have no output
            // layers (e.g., production mode with missing log-file rejects
            // during resolve, but the subscriber was already built with
            // no console and no file layer).
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    tracing::info!(
        worker_name = %config.worker_name,
        arch = %config.arch,
        server = %config.server_url,
        "starting cbsd-worker"
    );

    // Set up graceful shutdown.
    let state = Arc::new(ShutdownState::new());
    let _signal_handle = install_signal_handler(Arc::clone(&state));

    // Run the reconnection loop (returns on SIGTERM).
    reconnect_loop(&config, state).await;

    tracing::info!("cbsd-worker stopped");
}
