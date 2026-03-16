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

//! Worker liveness tracking types.

use cbsd_proto::Arch;

/// Server-assigned UUID for a WebSocket connection.
pub type ConnectionId = String;

/// Tracks the lifecycle state of a connected worker.
#[allow(dead_code)]
#[derive(Debug)]
pub enum WorkerState {
    /// Worker is connected and ready to accept builds.
    Connected {
        registered_worker_id: String,
        worker_name: String,
        arch: Arch,
        cores_total: u32,
        ram_total_mb: u64,
    },
    /// Worker disconnected; within the grace period for reconnection.
    Disconnected {
        since: tokio::time::Instant,
        registered_worker_id: String,
        worker_name: String,
        arch: Arch,
    },
    /// Worker announced graceful shutdown.
    Stopping {
        registered_worker_id: String,
        worker_name: String,
    },
    /// Worker is dead (grace period expired or unrecoverable).
    Dead,
}

impl WorkerState {
    /// Returns `true` only when the worker is connected and eligible for
    /// build dispatch.
    pub fn is_dispatch_eligible(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    /// Registered worker UUID, if available.
    pub fn registered_worker_id(&self) -> Option<&str> {
        match self {
            Self::Connected {
                registered_worker_id,
                ..
            }
            | Self::Disconnected {
                registered_worker_id,
                ..
            }
            | Self::Stopping {
                registered_worker_id,
                ..
            } => Some(registered_worker_id),
            Self::Dead => None,
        }
    }

    /// Human-readable display name, if available.
    #[allow(dead_code)]
    pub fn worker_name(&self) -> Option<&str> {
        match self {
            Self::Connected { worker_name, .. }
            | Self::Disconnected { worker_name, .. }
            | Self::Stopping { worker_name, .. } => Some(worker_name),
            Self::Dead => None,
        }
    }

    /// Architecture, if the worker is connected.
    pub fn arch(&self) -> Option<Arch> {
        match self {
            Self::Connected { arch, .. } | Self::Disconnected { arch, .. } => Some(*arch),
            Self::Stopping { .. } | Self::Dead => None,
        }
    }

    /// Short name for the current state (used in API responses).
    #[allow(dead_code)]
    pub fn state_name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::Disconnected { .. } => "disconnected",
            Self::Stopping { .. } => "stopping",
            Self::Dead => "dead",
        }
    }
}

/// Spawn a background task that sleeps for `grace_secs`, then checks if the
/// worker is still `Disconnected`. If so, transitions to `Dead` and handles
/// active builds per the dead-worker resolution table.
pub fn start_grace_period_monitor(
    state: &crate::app::AppState,
    connection_id: &str,
    grace_secs: u64,
) {
    let state = state.clone();
    let connection_id = connection_id.to_string();

    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(grace_secs)).await;

        // Check if the worker is still Disconnected.
        let still_disconnected = {
            let queue = state.queue.lock().await;
            matches!(
                queue.get_worker(&connection_id),
                Some(WorkerState::Disconnected { .. })
            )
        };

        if !still_disconnected {
            // Worker reconnected, was deregistered, or was already marked dead.
            return;
        }

        tracing::warn!(
            connection_id = %connection_id,
            grace_secs = grace_secs,
            "grace period expired — marking worker dead"
        );

        {
            let mut queue = state.queue.lock().await;
            queue.set_worker_state(&connection_id, WorkerState::Dead);
        }

        // Resolve active builds.
        crate::ws::handler::handle_worker_dead(&state, &connection_id).await;
    });
}
