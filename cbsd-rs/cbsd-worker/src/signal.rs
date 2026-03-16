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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinHandle;

/// Shared shutdown state used to coordinate graceful termination.
///
/// The signal handler sets `stopping` to true and notifies via `notify`.
/// The reconnection loop checks `stopping` to decide whether to reconnect.
pub struct ShutdownState {
    /// Set to `true` when SIGTERM is received.
    pub stopping: AtomicBool,

    /// Notified when `stopping` transitions to `true`.
    pub notify: tokio::sync::Notify,
}

impl ShutdownState {
    pub fn new() -> Self {
        Self {
            stopping: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Check whether shutdown has been requested.
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Relaxed)
    }
}

/// Install a SIGTERM handler that sets `state.stopping` and notifies waiters.
///
/// Returns the spawned task handle for the signal listener.
pub fn install_signal_handler(state: Arc<ShutdownState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
        sigterm.recv().await;
        tracing::info!("received SIGTERM, initiating graceful shutdown");
        state.stopping.store(true, Ordering::Relaxed);
        state.notify.notify_waiters();
    })
}
