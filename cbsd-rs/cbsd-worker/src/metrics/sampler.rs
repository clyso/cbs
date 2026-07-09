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

//! Per-connection metrics sampler task (design 021).
//!
//! Spawned once per WebSocket connection, but only when the worker has metrics
//! enabled *and* the server advertised `accepts_metrics`. It ticks on the push
//! interval, assembles a [`WorkerMessage::Metrics`] from the process-global host
//! and app sources, and `try_send`s it down a clone of the connection's
//! outbound channel — **never** `send().await`, so a slow/full transport can
//! never stall sampling or wedge the build path. A full channel drops the
//! sample and bumps `push_drops_total`. The task ends when the channel closes
//! (the connection dropped its receiver).
//!
//! ccache stats are sampled on a slower cadence and *carried forward* onto every
//! push, so the gauge stays fresh between the expensive `ccache` invocations.

use std::sync::Arc;
use std::time::Duration;

use cbsd_proto::ws::{AppMetrics, CcacheMetrics, WorkerMessage};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval};

use crate::build::supervisor::Supervisor;
use crate::metrics::{app, host};

/// Whether to start the sampler for a connection: both the worker's own switch
/// and the server's request must be set. Pure so the gate is unit-testable.
pub fn should_start(metrics_enabled: bool, accepts_metrics: bool) -> bool {
    metrics_enabled && accepts_metrics
}

/// `try_send` one message, counting a drop when the channel is full. Returns
/// `false` when the channel is closed (receiver gone) so the caller stops.
fn send_or_count(tx: &mpsc::Sender<WorkerMessage>, msg: WorkerMessage) -> bool {
    match tx.try_send(msg) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            app::record_push_drop();
            true
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

/// Run the sampler loop until the outbound channel closes. `out_tx` is a clone
/// of the connection's transport sender; dropping the connection's receiver ends
/// this task on the next tick.
pub async fn run_sampler(
    out_tx: mpsc::Sender<WorkerMessage>,
    supervisor: Arc<Supervisor>,
    push_interval: Duration,
    ccache_interval: Duration,
) {
    let mut ticker = interval(push_interval);
    // If the runtime stalls, skip missed ticks rather than burst-sending.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut carried_ccache: Option<CcacheMetrics> = None;
    let mut last_ccache: Option<Instant> = None;

    loop {
        ticker.tick().await;

        // Refresh ccache on its slower cadence; otherwise reuse the last value.
        let due = last_ccache.is_none_or(|t| t.elapsed() >= ccache_interval);
        if due {
            // The `ccache` subprocess is blocking work; keep it off the async
            // worker threads. A refreshed `None` (ccache absent) is carried as
            // `None`, dropping the series rather than freezing a stale value. A
            // panic in the blocking task is logged rather than silently dropped.
            let refreshed = match tokio::task::spawn_blocking(app::sample_ccache).await {
                Ok(result) => result,
                Err(err) => {
                    tracing::warn!(%err, "ccache sampling task panicked; dropping series");
                    None
                }
            };
            // Log availability transitions only, so an absent ccache warns once
            // per connection rather than on every refresh interval.
            let first = last_ccache.is_none();
            if refreshed.is_none() && (first || carried_ccache.is_some()) {
                tracing::warn!(
                    "ccache stats unavailable; cbsd_worker_ccache_* series will be \
                     omitted (is ccache installed and CCACHE_DIR set?)"
                );
            } else if refreshed.is_some() && !first && carried_ccache.is_none() {
                tracing::info!(
                    "ccache stats available again; resuming cbsd_worker_ccache_* series"
                );
            }
            carried_ccache = refreshed;
            last_ccache = Some(Instant::now());
        }

        let host = host::global().sample();
        let app = AppMetrics {
            ccache: carried_ccache.clone(),
            subprocess_exits: app::subprocess_exits(),
            spool_bytes: supervisor.spool_bytes().await,
            push_drops_total: app::push_drops(),
            sigkill_escalations_total: app::sigkill_escalations(),
        };
        let msg = WorkerMessage::Metrics {
            uptime_secs: app::uptime_secs(),
            host,
            app,
        };

        if !send_or_count(&out_tx, msg) {
            tracing::debug!("metrics transport closed; sampler stopping");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbsd_proto::ws::{AppMetrics, HostMetrics, SubprocessExitCounts};

    fn dummy_metrics_msg() -> WorkerMessage {
        WorkerMessage::Metrics {
            uptime_secs: 0,
            host: HostMetrics {
                cpu_busy_ratio: 0.0,
                load1: 0.0,
                mem_total_bytes: 0,
                mem_used_bytes: 0,
                mem_available_bytes: 0,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                filesystems: Vec::new(),
                disk_read_bytes_total: 0,
                disk_written_bytes_total: 0,
            },
            app: AppMetrics {
                ccache: None,
                subprocess_exits: SubprocessExitCounts {
                    success: 0,
                    failure: 0,
                    revoked: 0,
                },
                spool_bytes: 0,
                push_drops_total: 0,
                sigkill_escalations_total: 0,
            },
        }
    }

    #[test]
    fn should_start_requires_both_switches() {
        assert!(should_start(true, true));
        assert!(!should_start(false, true));
        assert!(!should_start(true, false));
        assert!(!should_start(false, false));
    }

    #[tokio::test]
    async fn full_channel_counts_a_drop_and_does_not_block() {
        // Capacity 1: first send fills it, second must drop (not block).
        let (tx, _rx) = mpsc::channel::<WorkerMessage>(1);
        assert!(send_or_count(&tx, dummy_metrics_msg()), "first send fits");

        let base = app::push_drops();
        // This would block on send().await; send_or_count must return promptly.
        assert!(
            send_or_count(&tx, dummy_metrics_msg()),
            "full channel is not closed, so the loop continues"
        );
        assert_eq!(app::push_drops() - base, 1, "a drop should be counted");
    }

    #[test]
    fn closed_channel_signals_stop() {
        let (tx, rx) = mpsc::channel::<WorkerMessage>(1);
        drop(rx);
        assert!(
            !send_or_count(&tx, dummy_metrics_msg()),
            "closed channel must signal the loop to stop"
        );
    }

    /// End-to-end: the full `run_sampler` loop (not just `send_or_count`) must
    /// terminate once the connection drops its receiver, so the per-connection
    /// task does not leak across reconnects. A regression turning the loop's
    /// `break` into `continue` would hang here and trip the timeout.
    #[tokio::test]
    async fn run_sampler_stops_when_transport_closes() {
        use std::sync::Arc;
        use std::time::Duration;

        use crate::build::supervisor::Supervisor;

        let tmp = tempfile::tempdir().expect("tempdir");
        let supervisor = Arc::new(Supervisor::new(tmp.path().to_path_buf()));
        let (tx, rx) = mpsc::channel::<WorkerMessage>(1);
        // Close the transport before the first tick: the sampler's `try_send`
        // sees a closed channel and the loop must break.
        drop(rx);

        let handle = tokio::spawn(run_sampler(
            tx,
            supervisor,
            Duration::from_millis(10),
            Duration::from_secs(60),
        ));

        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "run_sampler did not stop after the transport closed"
        );
        joined.unwrap().expect("run_sampler task panicked");
    }
}
