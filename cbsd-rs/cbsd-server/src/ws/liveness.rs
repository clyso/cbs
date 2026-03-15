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
#[derive(Debug)]
pub enum WorkerState {
    /// Worker is connected and ready to accept builds.
    Connected {
        worker_id: String,
        arch: Arch,
        cores_total: u32,
        ram_total_mb: u64,
    },
    /// Worker disconnected; within the grace period for reconnection.
    Disconnected {
        since: tokio::time::Instant,
        worker_id: String,
        arch: Arch,
    },
    /// Worker announced graceful shutdown.
    Stopping { worker_id: String },
    /// Worker is dead (grace period expired or unrecoverable).
    Dead,
}

impl WorkerState {
    /// Returns `true` only when the worker is connected and eligible for
    /// build dispatch.
    pub fn is_dispatch_eligible(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    /// Human-readable display label, if available.
    pub fn worker_id(&self) -> Option<&str> {
        match self {
            Self::Connected { worker_id, .. }
            | Self::Disconnected { worker_id, .. }
            | Self::Stopping { worker_id } => Some(worker_id),
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
    pub fn state_name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::Disconnected { .. } => "disconnected",
            Self::Stopping { .. } => "stopping",
            Self::Dead => "dead",
        }
    }
}
