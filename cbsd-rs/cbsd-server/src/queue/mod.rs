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

//! In-memory build queue with three priority lanes.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cbsd_proto::{Arch, BuildDescriptor, BuildId, Priority};
use serde::Serialize;

use crate::ws::liveness::{ConnectionId, WorkerState};

/// A build that has been dispatched to a worker and is currently active.
#[allow(dead_code)]
pub struct ActiveBuild {
    pub build_id: i64,
    pub connection_id: ConnectionId,
    pub dispatched_at: tokio::time::Instant,
    pub trace_id: String,
    pub descriptor: BuildDescriptor,
    pub priority: Priority,
    /// Cancel token for the dispatch ack timeout. Cancelled when
    /// `build_accepted` is received.
    pub ack_cancel: tokio_util::sync::CancellationToken,
}

/// Summary information about a connected worker (returned by `GET /workers`).
#[derive(Debug, Serialize)]
pub struct WorkerInfo {
    pub connection_id: ConnectionId,
    pub worker_id: String,
    pub arch: Arch,
    pub state_name: String,
    pub current_build_id: Option<i64>,
}

/// A build waiting in the queue.
#[allow(dead_code)]
pub struct QueuedBuild {
    pub build_id: BuildId,
    pub priority: Priority,
    pub descriptor: BuildDescriptor,
    pub user_email: String,
    pub queued_at: i64,
}

/// Three-lane priority build queue: high > normal > low.
///
/// Also tracks active builds and connected workers.
pub struct BuildQueue {
    high: VecDeque<QueuedBuild>,
    normal: VecDeque<QueuedBuild>,
    low: VecDeque<QueuedBuild>,
    /// Builds currently dispatched to workers, keyed by build ID.
    pub active: HashMap<i64, ActiveBuild>,
    /// Connected workers, keyed by server-assigned connection UUID.
    pub workers: HashMap<ConnectionId, WorkerState>,
}

/// Thread-safe handle to the build queue.
pub type SharedBuildQueue = Arc<tokio::sync::Mutex<BuildQueue>>;

impl BuildQueue {
    /// Create an empty queue with no pending builds.
    pub fn new() -> Self {
        Self {
            high: VecDeque::new(),
            normal: VecDeque::new(),
            low: VecDeque::new(),
            active: HashMap::new(),
            workers: HashMap::new(),
        }
    }

    /// Push a build to the back of the appropriate priority lane.
    pub fn enqueue(&mut self, build: QueuedBuild) {
        self.lane_mut(build.priority).push_back(build);
    }

    /// Push a build to the front of the appropriate priority lane.
    /// Used for re-queue after rejection or timeout.
    pub fn enqueue_front(&mut self, build: QueuedBuild) {
        self.lane_mut(build.priority).push_front(build);
    }

    /// Pop the next pending build from the highest non-empty lane.
    pub fn next_pending(&mut self) -> Option<QueuedBuild> {
        if let Some(build) = self.high.pop_front() {
            return Some(build);
        }
        if let Some(build) = self.normal.pop_front() {
            return Some(build);
        }
        self.low.pop_front()
    }

    /// Search all lanes and remove the build with the given ID.
    pub fn remove_by_id(&mut self, build_id: BuildId) -> Option<QueuedBuild> {
        if let Some(build) = remove_from_lane(&mut self.high, build_id) {
            return Some(build);
        }
        if let Some(build) = remove_from_lane(&mut self.normal, build_id) {
            return Some(build);
        }
        remove_from_lane(&mut self.low, build_id)
    }

    /// Return the number of pending builds per lane: (high, normal, low).
    pub fn pending_counts(&self) -> (usize, usize, usize) {
        (self.high.len(), self.normal.len(), self.low.len())
    }

    /// Check if a build with the given ID is in the queue.
    #[allow(dead_code)]
    pub fn contains(&self, build_id: BuildId) -> bool {
        lane_contains(&self.high, build_id)
            || lane_contains(&self.normal, build_id)
            || lane_contains(&self.low, build_id)
    }

    /// Get a mutable reference to the lane for the given priority.
    fn lane_mut(&mut self, priority: Priority) -> &mut VecDeque<QueuedBuild> {
        match priority {
            Priority::High => &mut self.high,
            Priority::Normal => &mut self.normal,
            Priority::Low => &mut self.low,
        }
    }

    // -- Worker management --

    /// Register a worker with the given connection ID and state.
    pub fn register_worker(&mut self, connection_id: ConnectionId, state: WorkerState) {
        self.workers.insert(connection_id, state);
    }

    /// Remove a worker by connection ID. Returns the previous state if present.
    #[allow(dead_code)]
    pub fn remove_worker(&mut self, connection_id: &str) -> Option<WorkerState> {
        self.workers.remove(connection_id)
    }

    /// Look up a worker by connection ID.
    pub fn get_worker(&self, connection_id: &str) -> Option<&WorkerState> {
        self.workers.get(connection_id)
    }

    /// Replace the state of an existing worker.
    pub fn set_worker_state(&mut self, connection_id: &str, state: WorkerState) {
        if let Some(entry) = self.workers.get_mut(connection_id) {
            *entry = state;
        }
    }

    /// Return summary information for all workers (for `GET /api/workers`).
    pub fn connected_workers(&self) -> Vec<WorkerInfo> {
        self.workers
            .iter()
            .filter_map(|(cid, state)| {
                let worker_id = state.worker_id()?.to_string();
                let arch = state.arch()?;
                // Look up whether this worker has an active build
                let current_build_id = self
                    .active
                    .values()
                    .find(|ab| ab.connection_id == *cid)
                    .map(|ab| ab.build_id);
                Some(WorkerInfo {
                    connection_id: cid.clone(),
                    worker_id,
                    arch,
                    state_name: state.state_name().to_string(),
                    current_build_id,
                })
            })
            .collect()
    }
}

/// Remove the first build with the given ID from a lane.
fn remove_from_lane(lane: &mut VecDeque<QueuedBuild>, build_id: BuildId) -> Option<QueuedBuild> {
    let pos = lane.iter().position(|b| b.build_id == build_id)?;
    lane.remove(pos)
}

/// Check if a lane contains a build with the given ID.
#[allow(dead_code)]
fn lane_contains(lane: &VecDeque<QueuedBuild>, build_id: BuildId) -> bool {
    lane.iter().any(|b| b.build_id == build_id)
}

#[cfg(test)]
mod tests {
    use cbsd_proto::build::{
        BuildComponent, BuildDestImage, BuildSignedOffBy, BuildTarget, VersionType,
    };
    use cbsd_proto::{Arch, BuildDescriptor};

    use super::*;

    fn sample_descriptor() -> BuildDescriptor {
        BuildDescriptor {
            version: "19.2.3".to_string(),
            channel: "ces-devel".to_string(),
            version_type: VersionType::Dev,
            signed_off_by: BuildSignedOffBy {
                user: "Alice".to_string(),
                email: "alice@clyso.com".to_string(),
            },
            dst_image: BuildDestImage {
                name: "harbor.clyso.com/ces-devel/ceph".to_string(),
                tag: "v19.2.3-dev.1".to_string(),
            },
            components: vec![BuildComponent {
                name: "ceph".to_string(),
                git_ref: "v19.2.3".to_string(),
                repo: None,
            }],
            build: BuildTarget {
                distro: "rockylinux".to_string(),
                os_version: "el9".to_string(),
                artifact_type: "rpm".to_string(),
                arch: Arch::X86_64,
            },
        }
    }

    fn queued(id: i64, priority: Priority) -> QueuedBuild {
        QueuedBuild {
            build_id: BuildId(id),
            priority,
            descriptor: sample_descriptor(),
            user_email: "alice@clyso.com".to_string(),
            queued_at: 1000,
        }
    }

    #[test]
    fn enqueue_and_next_respects_priority() {
        let mut q = BuildQueue::new();
        q.enqueue(queued(1, Priority::Low));
        q.enqueue(queued(2, Priority::High));
        q.enqueue(queued(3, Priority::Normal));

        assert_eq!(q.next_pending().unwrap().build_id, BuildId(2));
        assert_eq!(q.next_pending().unwrap().build_id, BuildId(3));
        assert_eq!(q.next_pending().unwrap().build_id, BuildId(1));
        assert!(q.next_pending().is_none());
    }

    #[test]
    fn enqueue_front_places_at_head() {
        let mut q = BuildQueue::new();
        q.enqueue(queued(1, Priority::Normal));
        q.enqueue_front(queued(2, Priority::Normal));

        assert_eq!(q.next_pending().unwrap().build_id, BuildId(2));
        assert_eq!(q.next_pending().unwrap().build_id, BuildId(1));
    }

    #[test]
    fn remove_by_id_finds_and_removes() {
        let mut q = BuildQueue::new();
        q.enqueue(queued(1, Priority::Normal));
        q.enqueue(queued(2, Priority::High));
        q.enqueue(queued(3, Priority::Low));

        let removed = q.remove_by_id(BuildId(2));
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().build_id, BuildId(2));
        assert!(!q.contains(BuildId(2)));
        assert!(q.contains(BuildId(1)));
        assert!(q.contains(BuildId(3)));
    }

    #[test]
    fn remove_by_id_returns_none_for_missing() {
        let mut q = BuildQueue::new();
        q.enqueue(queued(1, Priority::Normal));
        assert!(q.remove_by_id(BuildId(99)).is_none());
    }

    #[test]
    fn pending_counts_tracks_all_lanes() {
        let mut q = BuildQueue::new();
        assert_eq!(q.pending_counts(), (0, 0, 0));

        q.enqueue(queued(1, Priority::High));
        q.enqueue(queued(2, Priority::High));
        q.enqueue(queued(3, Priority::Normal));
        q.enqueue(queued(4, Priority::Low));

        assert_eq!(q.pending_counts(), (2, 1, 1));
    }

    #[test]
    fn contains_checks_all_lanes() {
        let mut q = BuildQueue::new();
        assert!(!q.contains(BuildId(1)));

        q.enqueue(queued(1, Priority::Low));
        assert!(q.contains(BuildId(1)));
        assert!(!q.contains(BuildId(2)));
    }
}
