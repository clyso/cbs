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
use tracing_subscriber::EnvFilter;

use crate::config::WorkerConfig;
use crate::signal::{ShutdownState, install_signal_handler};
use crate::ws::connection::reconnect_loop;

/// CBS build worker — connects to the CBS server via WebSocket and executes
/// build jobs.
#[derive(Parser)]
#[command(name = "cbsd-worker", version, about)]
struct Cli {
    /// Path to the worker configuration YAML file.
    #[arg(short, long, default_value = "worker.yaml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Load configuration.
    let config = match WorkerConfig::load(&cli.config) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    // Initialize tracing (respects RUST_LOG env var, defaults to info).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!(
        worker_id = %config.worker_id,
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
