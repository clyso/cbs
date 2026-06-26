# 021 — Metrics: Wire Protocol & Worker Collector

## Status

Draft v1. First of three design documents that turn accepted proposal
[002](../proposals/002-20260625T0839-metrics-observability-websocket-push.md)
(metrics over the worker WebSocket) into an implementable spec. This document
covers the **`cbsd-proto` wire additions** and the **worker-side collector**.
Companions:

- **022 — Server exporter & aggregation** (`cbsd-server`: `/metrics`, the
  server-owned registry, and how pushed snapshots are written into the facade
  with gauge idle-timeout staleness).
- **023 — Deployment & dashboards** (config wiring, podman-compose, provisioned
  Grafana).

Rationale for the architecture (why push over the WebSocket, why no per-host
daemons) lives in proposal 002 and is not repeated here. This document is the
**what to build** for the worker and the shared types.

## Scope

In scope: the `WorkerMessage::Metrics` message, the `Welcome.accepts_metrics`
capability flag, the worker `sysinfo` host sampler, the worker app-metric
sources (ccache, subprocess exits, output spool), the per-connection sampler
task and its backpressure rules, and the worker `metrics:` config.

Out of scope (see 022): the server's parsing, label-stamping, aggregation,
staleness, and exposition of these snapshots.

## Wire protocol additions (`cbsd-proto/src/ws.rs`)

### `WorkerMessage::Metrics` (worker → server)

A new variant on the existing `WorkerMessage` enum, which is
`#[serde(tag = "type", rename_all = "snake_case")]` — so the wire tag is
`"metrics"`. The payload is a **typed snapshot**, not opaque text (proposal 002
§"Worker → server metrics protocol").

```rust
/// Periodic metrics snapshot (protocol v2, additive). Pushed by the worker
/// on an interval while connected, only if the server advertised
/// `accepts_metrics` in `Welcome`. Point-in-time: never spooled or replayed.
Metrics {
    /// Worker monotonic uptime in seconds at sample time. Lets the server
    /// detect a worker-process restart (uptime regressed) independently of
    /// counter resets.
    uptime_secs: u64,
    host: HostMetrics,
    app: AppMetrics,
},
```

```rust
/// Host resource snapshot sampled in-process via `sysinfo`. All gauges are
/// point-in-time; cumulative fields are documented as such. Field coverage
/// is the dashboard-driven subset (proposal 002 §"Worker host-metric
/// collection"); exact `sysinfo` calls are pinned in implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HostMetrics {
    /// Overall CPU busy fraction in [0.0, 1.0] since the previous sample.
    pub cpu_busy_ratio: f64,
    /// 1-minute load average.
    pub load1: f64,
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub mem_available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    /// Per-mount filesystem usage for the volumes that matter
    /// (e.g. `/cbs/scratch`, `/cbs/ccache`, container storage). Keyed by
    /// mount path; the server turns the key into a `mount` label.
    pub filesystems: Vec<FilesystemUsage>,
    /// Cumulative disk bytes read/written since worker start (host-wide).
    /// Republished as counters by the server.
    pub disk_read_bytes_total: u64,
    pub disk_written_bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FilesystemUsage {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}
```

```rust
/// Application-level metrics only the worker can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AppMetrics {
    /// ccache on-disk size and effectiveness, from `ccache -s` (sampled on a
    /// slower cadence than the push; carried forward between refreshes).
    /// `None` if ccache is unavailable/disabled on this worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccache: Option<CcacheMetrics>,
    /// Cumulative build-subprocess terminations by classified exit kind
    /// since worker start (success / failure / revoked). Republished as a
    /// counter with a `result` label (same values as the server's
    /// `cbsd_build_results_total{result}`, for a consistent label vocabulary).
    pub subprocess_exits: SubprocessExitCounts,
    /// Current output-spool bytes buffered for the active build (0 when idle).
    /// The 64 MiB spool budget lives in the supervisor; this is its fill.
    pub spool_bytes: u64,
    /// Cumulative count of metrics snapshots dropped by `try_send` because the
    /// outbound channel was full (NF2). Rides each push so sustained drops are
    /// visible. Resets on worker restart (a normal counter reset).
    pub push_drops_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CcacheMetrics {
    pub size_bytes: u64,
    pub max_bytes: u64,
    /// Cache hit ratio in [0.0, 1.0] as reported by `ccache -s`.
    pub hit_ratio: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SubprocessExitCounts {
    pub success: u64,
    pub failure: u64,
    pub revoked: u64,
}
```

**Tag exhaustiveness (net-new, no free enforcement).** Unlike `ServerMessage` —
which has a `ServerMessageTag` enum, a `from_message` mapping, and a
parity/exhaustiveness test (`cbsd-proto/src/ws.rs:581`, `:596`, `:717`) —
`WorkerMessage` currently has **no** tag enum and only per-variant round-trip
tests. So no existing test will flag a missing `Metrics` arm. The `metrics`
round-trip test below is the actual coverage. Because the entire 022/023 worker-
metrics surface now hangs off this single new variant, **add a
`WorkerMessageTag` mirror** (enum + `from_message` mapping +
parity/exhaustiveness test) in this work, matching `ServerMessage`'s
`ServerMessageTag` (`ws.rs:581`, `:596`, `:717`). It is net-new — not free — but
it is cheap, and it makes every _future_ `WorkerMessage` addition fail the build
until handled, rather than silently slipping past per-variant tests as `Metrics`
nearly could.

**Why cumulative + uptime.** Counters (`subprocess_exits`, `disk_*_bytes_total`,
`push_drops_total`) are **cumulative since worker process start**; the server
republishes them via the `metrics` facade counter handle's `.absolute(v)` (022),
and `rate()` tolerates the reset when a worker restarts. `uptime_secs` lets the
server distinguish a counter reset caused by a restart from a transient gap, and
is a useful `cbsd_worker_uptime_seconds` gauge in its own right.

### `Welcome.accepts_metrics` (server → worker capability)

The worker must learn whether the server understands `Metrics` before sending
any, to avoid one parse-`warn!` per push against a not-yet-upgraded server
during a rolling upgrade (proposal 002, finding N1). Add one field to the
existing `ServerMessage::Welcome` struct variant:

```rust
Welcome {
    protocol_version: u32,
    connection_id: String,
    grace_period_secs: u64,
    /// True if this server consumes `WorkerMessage::Metrics`. Additive and
    /// MUST be `#[serde(default)]`: a pre-upgrade server omits the field, and
    /// an upgraded worker deserializing that `Welcome` on the strict path
    /// would otherwise fail to connect. Absent ⇒ `false` ⇒ worker stays
    /// silent (the desired degraded behavior).
    #[serde(default)]
    accepts_metrics: bool,
},
```

This is a bare `#[serde(default)] bool`, **not** `Option<bool>`: "supports
metrics" is plainly false/true and `#[serde(default)]` already supplies the
absent ⇒ `false` semantics. (The `BuildRevoke.reason` precedent at `ws.rs:48-52`
uses `Option` only because a revoke reason is genuinely tri-state.) No
`protocol_version` bump: the server still hard-rejects `protocol_version != 2`
(`cbsd-server/src/ws/handler.rs:141`); this rides within v2 as a
backward/forward-compatible additive field.

### Required serde tests

- `metrics` round-trips: `WorkerMessage::Metrics { .. }` → JSON (tag
  `"metrics"`) → back, value-equal.
- **Missing-field compat (N1):** a `Welcome` JSON object _without_
  `accepts_metrics` deserializes with `accepts_metrics == false`.
- Tag-parity test covers the new `Metrics` variant.
- An old peer ignores unknown variants/fields (the enum carries no
  `deny_unknown_fields`; an existing test asserts its absence — keep it green).

## Worker collector

### Config (`cbsd-worker/src/config.rs`)

Add a `metrics` section to `WorkerConfig` (kebab-case keys, `#[serde(default)]`
parent so the whole section is optional), mirroring the existing `LoggingConfig`
pattern:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct WorkerMetricsConfig {
    /// Master switch. Default true: a metrics-capable worker pushes if the
    /// server also advertises `accepts_metrics`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Seconds between metric snapshots/pushes. Default 15.
    #[serde(default = "default_push_interval")]
    pub push_interval_secs: u64,
    /// Seconds between the slower `ccache -s` refresh. Default 60.
    #[serde(default = "default_ccache_interval")]
    pub ccache_interval_secs: u64,
}
```

Resolved onto `ResolvedWorkerConfig` like the other operational fields. No bind
address — the worker exposes no endpoint.

### Host sampler (`sysinfo`)

Add `sysinfo` to `cbsd-worker/Cargo.toml`, pinned to current stable (≈ 0.3x). A
`HostSampler` wraps a single long-lived `sysinfo::System` (refreshing in place
is far cheaper than re-instantiating) and produces a `HostMetrics` per call. The
`HostSampler` is **process-global** (created once at startup, behind a `Mutex`),
**not** owned by the per-connection sampler task — so its warmed CPU-delta state
survives reconnects rather than being zeroed each time a new task spawns. The
per-connection task borrows it per tick.

- `cpu_busy_ratio` — `refresh_cpu_usage()` then average per-CPU usage / 100. The
  first sample after start is discarded/zeroed (sysinfo needs two refreshes for
  a delta).
- memory/swap — `refresh_memory()`.
- `filesystems` — `Disks::new_with_refreshed_list()`, filtered to the configured
  mounts of interest (default: the worker's scratch, ccache, and container
  storage dirs; derived from existing config paths where available).
- `load1` — `System::load_average().one` (Linux).
- `disk_*_bytes_total` — per-disk cumulative IO from `Disk::usage()`
  (`total_read_bytes` / `total_written_bytes`), summed across disks. On current
  `sysinfo` these are first-class on Linux, so the earlier `/proc` hedge is a
  last resort only; pin the version and confirm the fields at implementation.
  Any field that genuinely cannot be sourced is documented and omitted, not
  faked.

The sampler is synchronous and cheap; run it inline on the push tick.

### App-metric sources

- **ccache** — shell out to `ccache -s` (machine-readable `-v`/`--print-stats`
  where available) on the slower `ccache_interval_secs` cadence; parse
  size/max/hit-ratio. The worker already mounts `/cbs/ccache`. Cache the last
  `CcacheMetrics` and carry it forward between refreshes. `None` if the binary
  or cache dir is absent. **`ccache_interval_secs` governs only the `ccache -s`
  shell-out, never the push: the cached value rides _every_ push.** This is
  load-bearing for staleness, not just a cost optimization — the server exposes
  ccache as gauges under the `GAUGE` idle-timeout (022), so if the worker only
  attached ccache on refresh ticks the gauge would idle out whenever
  `ccache_interval_secs > stale_after_secs`. Re-sending the carried-forward
  value each push keeps `set()` (hence the idle timer) fresh.
- **subprocess exits** — the executor already classifies child exit codes into
  success/failure/revoked (`cbsd-worker/src/build/executor.rs`,
  `classify_exit_code`). Increment a process-global `SubprocessExitCounts` (e.g.
  `AtomicU64` trio) at the point the build result is finalized.
- **spool_bytes** — read the supervisor's current spool fill for the active
  build (the 64 MiB budget tracker in `cbsd-worker/src/build/supervisor.rs`); 0
  when idle.
- **push_drops_total** — an `AtomicU64` incremented whenever `try_send` (below)
  returns `Full`.

These cumulative counters live process-global (not per-connection) so they
survive reconnects; only the worker _process_ restart resets them.

### Per-connection sampler task & backpressure (N2/N3, F4)

The outbound sender `out_tx` is **per-connection**: created inside
`run_connection` and cloned to the supervisor
(`cbsd-worker/src/ws/handler.rs:91-92`). The sampler must therefore be a
**per-connection task**, spawned in `run_connection` after a `Welcome` with
`accepts_metrics == true`, holding its own `out_tx.clone()` — a sibling of the
supervisor's clone, never routed through the supervisor.

Rules (proposal 002 §"Shared-channel backpressure", §"Sampler lifecycle"):

- **Gate on capability + config.** Spawn only if `metrics.enabled` _and_
  `Welcome.accepts_metrics`. Otherwise no sampler exists and nothing is sent.
- **`try_send`, drop on full.** Each tick build a `WorkerMessage::Metrics` and
  `out_tx.try_send(..)`. On `Full`, increment `push_drops_total` and drop the
  sample — never `send().await` (blocking would back-pressure into build
  handling). The single channel is `OUTPUT_CHANNEL_CAPACITY = 64`
  (`ws/handler.rs:50`).
- **Bypass the supervisor.** Metrics never enter `send_or_spool`, which drops
  messages when no build is active (`build/supervisor.rs:269-275`) — routing
  metrics through it would lose exactly the idle-worker host stats we want.
- **Lifecycle = connection.** The task is cancelled (or exits on `try_send`
  returning `Closed`) when the connection drops, alongside transport teardown.
  No long-lived sampler holding a swappable sender. "Detached" = the connection
  and its task are gone; reconnect spawns a fresh sampler. Metrics are never
  spooled or replayed.

### Tests

- `try_send` on a saturated 64-cap channel increments `push_drops_total` and
  does not block.
- Sampler is not spawned when `accepts_metrics == false` or
  `metrics.enabled == false`.
- Sampler task terminates when its connection's `out_rx` is dropped.
- `HostSampler` returns a plausible non-empty snapshot on a second sample
  (first-sample CPU zeroing honored).
- Subprocess-exit counters increment per classified result.

## Open items deferred to implementation

- Exact `sysinfo` version and the precise per-field call/`/proc` fallback map.
- `ccache` stats parsing format across ccache versions in the worker image.
- Which mount paths are sampled by default and how they are discovered from
  existing worker config/build paths.
