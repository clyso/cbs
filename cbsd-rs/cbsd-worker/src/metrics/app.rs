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

//! Application metric sources owned by the worker process (design 021).
//!
//! These are **process-global** so they survive WebSocket reconnects and reset
//! only on a worker-process restart — matching the server's counter semantics
//! (`.absolute(v)`, reset visible as an uptime regression). Build-subprocess
//! outcomes and dropped-push counts are atomics bumped from the build and
//! sampler paths; ccache stats are sampled on demand from `ccache`.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use cbsd_proto::ws::{BuildFinishedStatus, CcacheMetrics, SubprocessExitCounts};
use std::sync::OnceLock;

static SUCCESS: AtomicU64 = AtomicU64::new(0);
static FAILURE: AtomicU64 = AtomicU64::new(0);
static REVOKED: AtomicU64 = AtomicU64::new(0);
static PUSH_DROPS: AtomicU64 = AtomicU64::new(0);
static SIGKILL_ESCALATIONS: AtomicU64 = AtomicU64::new(0);

/// Process start instant, used to derive `uptime_secs`. Captured on first read
/// rather than at a fixed init point so it needs no main()-side wiring.
static STARTED_AT: OnceLock<Instant> = OnceLock::new();

/// Record one finished build subprocess under its classified outcome. Called
/// from the build output path at the single classification site so every
/// subprocess termination is counted once.
pub fn record_subprocess_exit(status: BuildFinishedStatus) {
    let counter = match status {
        BuildFinishedStatus::Success => &SUCCESS,
        BuildFinishedStatus::Failure => &FAILURE,
        BuildFinishedStatus::Revoked => &REVOKED,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the cumulative subprocess-exit tally.
pub fn subprocess_exits() -> SubprocessExitCounts {
    SubprocessExitCounts {
        success: SUCCESS.load(Ordering::Relaxed),
        failure: FAILURE.load(Ordering::Relaxed),
        revoked: REVOKED.load(Ordering::Relaxed),
    }
}

/// Record that a metrics push was dropped because the outbound channel was
/// full. A nonzero value tells operators the push path is the bottleneck.
pub fn record_push_drop() {
    PUSH_DROPS.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the cumulative dropped-push count.
pub fn push_drops() -> u64 {
    PUSH_DROPS.load(Ordering::Relaxed)
}

/// Record one SIGTERM→SIGKILL escalation on a build subprocess. Called from the
/// executor when the escalation timer fires and a SIGKILL is sent.
pub fn record_sigkill_escalation() {
    SIGKILL_ESCALATIONS.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the cumulative SIGKILL-escalation count.
pub fn sigkill_escalations() -> u64 {
    SIGKILL_ESCALATIONS.load(Ordering::Relaxed)
}

/// Seconds since this worker process started.
pub fn uptime_secs() -> u64 {
    STARTED_AT.get_or_init(Instant::now).elapsed().as_secs()
}

/// Sample ccache statistics via `ccache --print-stats` (the machine-readable
/// format stable since ccache 4.0). Returns `None` when ccache is absent or the
/// output cannot be parsed — the caller then omits the ccache series entirely
/// rather than reporting zeros.
pub fn sample_ccache() -> Option<CcacheMetrics> {
    let output = match Command::new("ccache").arg("--print-stats").output() {
        Ok(output) => output,
        Err(err) => {
            tracing::debug!(%err, "failed to run `ccache --print-stats`");
            return None;
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!(
            status = %output.status,
            stderr = %stderr.trim(),
            "`ccache --print-stats` exited unsuccessfully"
        );
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parsed = parse_print_stats(&text);
    if parsed.is_none() {
        tracing::debug!("`ccache --print-stats` output lacks the cache size fields");
    }
    parsed
}

/// Parse the `key\tvalue` lines of `ccache --print-stats`. Sizes are reported
/// in kibibytes; hit ratio is derived from the direct/preprocessed hit and miss
/// tallies. Returns `None` if the size fields are missing (a different ccache
/// build); a missing hit/miss simply counts as zero.
fn parse_print_stats(text: &str) -> Option<CcacheMetrics> {
    let mut size_kib: Option<u64> = None;
    let mut max_kib: Option<u64> = None;
    let mut direct_hit = 0u64;
    let mut preprocessed_hit = 0u64;
    let mut miss = 0u64;

    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(val)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(num) = val.parse::<u64>() else {
            continue;
        };
        match key {
            "cache_size_kibibyte" => size_kib = Some(num),
            "max_cache_size_kibibyte" => max_kib = Some(num),
            "direct_cache_hit" => direct_hit = num,
            "preprocessed_cache_hit" => preprocessed_hit = num,
            "cache_miss" => miss = num,
            _ => {}
        }
    }

    let hits = direct_hit + preprocessed_hit;
    let lookups = hits + miss;
    // No lookups yet ⇒ ratio is undefined; report 0.0 rather than NaN so the
    // gauge stays renderable.
    let hit_ratio = if lookups == 0 {
        0.0
    } else {
        hits as f64 / lookups as f64
    };

    Some(CcacheMetrics {
        size_bytes: size_kib? * 1024,
        max_bytes: max_kib? * 1024,
        hit_ratio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_counters_increment_per_result() {
        // Atomics are process-global; capture a baseline so the test is
        // order-independent within the shared process.
        let base = subprocess_exits();
        record_subprocess_exit(BuildFinishedStatus::Success);
        record_subprocess_exit(BuildFinishedStatus::Success);
        record_subprocess_exit(BuildFinishedStatus::Failure);
        record_subprocess_exit(BuildFinishedStatus::Revoked);
        let now = subprocess_exits();
        assert_eq!(now.success - base.success, 2);
        assert_eq!(now.failure - base.failure, 1);
        assert_eq!(now.revoked - base.revoked, 1);
    }

    #[test]
    fn push_drop_counter_increments() {
        let base = push_drops();
        record_push_drop();
        assert_eq!(push_drops() - base, 1);
    }

    #[test]
    fn sigkill_escalation_counter_increments() {
        let base = sigkill_escalations();
        record_sigkill_escalation();
        record_sigkill_escalation();
        assert_eq!(sigkill_escalations() - base, 2);
    }

    #[test]
    fn parse_print_stats_computes_size_and_ratio() {
        let sample = "\
cache_size_kibibyte\t1024
max_cache_size_kibibyte\t2048
direct_cache_hit\t30
preprocessed_cache_hit\t10
cache_miss\t10
";
        let m = parse_print_stats(sample).expect("parse");
        assert_eq!(m.size_bytes, 1024 * 1024);
        assert_eq!(m.max_bytes, 2048 * 1024);
        // 40 hits / 50 lookups = 0.8
        assert!((m.hit_ratio - 0.8).abs() < 1e-9, "ratio: {}", m.hit_ratio);
    }

    #[test]
    fn parse_print_stats_missing_sizes_is_none() {
        assert!(parse_print_stats("direct_cache_hit\t5\n").is_none());
    }

    #[test]
    fn parse_print_stats_zero_lookups_is_zero_ratio() {
        let sample = "cache_size_kibibyte\t0\nmax_cache_size_kibibyte\t0\n";
        let m = parse_print_stats(sample).expect("parse");
        assert_eq!(m.hit_ratio, 0.0);
    }
}
