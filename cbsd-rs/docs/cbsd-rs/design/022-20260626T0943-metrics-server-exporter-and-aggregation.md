# 022 — Metrics: Server Exporter & Aggregation

## Status

Draft v1. Second of three design documents implementing accepted proposal
[002](../proposals/002-20260625T0839-metrics-observability-websocket-push.md).
This document covers the **`cbsd-server`** side: the `/metrics` endpoint and its
configurable bind, the server-owned metric registry (names, types, labels,
buckets), where each metric is emitted, and how pushed `WorkerMessage::Metrics`
snapshots (defined in
[021](021-20260626T0942-metrics-protocol-and-worker-collector.md)) are written
into the facade and kept fresh via the recorder's gauge idle-timeout.

Companions: 021 (wire protocol + worker collector), 023 (deployment +
dashboards). Architecture rationale is in proposal 002.

## One facade, idle-expiring worker gauges

All metrics — server-owned and pushed-worker — are recorded through a **single**
`metrics` facade + `metrics-exporter-prometheus` recorder, and `/metrics` is
just `PrometheusHandle::render()`. There is no custom exposition path and no
hand-rolled cache. (An earlier draft used a custom `WorkerMetricsCache` on the
belief that the recorder could not expire individual labeled series; it can —
see below.)

The one subtlety the pushed-worker metrics raise is **staleness**: a worker that
stops pushing must not leave its CPU/memory gauges frozen at a stale value
forever. `metrics-exporter-prometheus` solves this directly with
`PrometheusBuilder::idle_timeout(MetricKindMask, Option<Duration>)`, which drops
any series (keyed by name + labels) not updated within the timeout. Configure it
for **gauges only**:

- **Worker gauges** (`cbsd_worker_host_*`, ccache, spool, uptime) are `set()`
  once per push. When a worker goes silent no updates arrive, and the gauge
  series is pruned once its last update is older than `stale_after` — exactly
  the desired disappearance.
- **Server-owned gauges** (queue depth, pool) are re-set every `gauge_refresh`
  seconds (≤ `stale_after`), so they never idle out.

**Pruning is render/upkeep-driven, not wall-clock (B1).** In
`metrics-exporter-prometheus`, idle series are removed only when output is
generated or upkeep is run — "metrics will not be removed unless a request has
been made recently enough"
([docs.rs](https://docs.rs/metrics-exporter-prometheus/latest/metrics_exporter_prometheus/struct.PrometheusBuilder.html#method.idle_timeout)).
So pruning does **not** happen just because a gauge stopped being `set()`. Two
consequences the implementation must honor:

- Each scrape calls `PrometheusHandle::render()`, which performs the prune; at a
  15 s scrape interval a 45 s-idle worker gauge is gone by the next scrape.
- To make pruning independent of scrape liveness, spawn a small **upkeep task**
  that calls `handle.run_upkeep()` every `gauge_refresh` seconds (the same task
  that re-sets the server-owned gauges is the natural home). Without it, a
  paused scraper would let stale worker gauges linger until scraping resumes.

This also **dissolves the reconnect-race** that an earlier draft had to guard:
series are keyed by their `worker` label value (the stable
`registered_worker_id`), so a reconnecting worker keeps updating the same
series; nothing is keyed by `connection_id` and no liveness-driven eviction is
needed (resolves proposal 002 F3 with zero custom code).

> **Worker label policy.** Worker-labeled **counters/histograms**
> (`cbsd_build_duration_seconds`, `cbsd_build_results_total`,
> `cbsd_worker_subprocess_exits_total`, …) are deliberately NOT idle-expired —
> the idle timeout is set for the `GAUGE` kind only. A decommissioned worker
> leaves a benign flat / zero-`rate()` series, which is correct for cumulative
> data and keeps server-owned counters from ever being pruned. Only **gauges of
> live state** must disappear, and `idle_timeout(GAUGE, …)` handles exactly
> those. This is the core design decision of this document.

## Dependencies & recorder setup

Add to `cbsd-server/Cargo.toml`: `metrics`, `metrics-exporter-prometheus`,
pinned to current stable (≈ `metrics-exporter-prometheus` 0.18). At startup,
before any metric is emitted:

```rust
let handle = PrometheusBuilder::new()
    .set_buckets_for_metric(Matcher::Full("cbsd_build_duration_seconds".into()),
                            &BUILD_DURATION_BUCKETS)?
    // … one per histogram family (buckets below) …
    // Expire idle GAUGE series after `stale_after` so a silent worker's host
    // gauges disappear; GAUGE-only so server-owned counters are never pruned.
    .idle_timeout(MetricKindMask::GAUGE, Some(stale_after))
    .install_recorder()?;          // global recorder; returns the render handle
```

`handle` (a `PrometheusHandle`) is the only state `/metrics` needs; store it in
a new `MetricsState` held by `AppState` as `Option<MetricsState>` (`None` when
`metrics.enabled == false`, in which case no recorder is installed and no
endpoint is served):

```rust
pub struct MetricsState {
    pub handle: PrometheusHandle,
}
```

Both server-owned and pushed-worker metrics render through `handle.render()` —
there is no second exposition path.

## Configuration (`cbsd-server/src/config.rs`)

New optional `metrics` section (kebab-case, `#[serde(default)]` parent), per the
"separate bind, but configurable" decision:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MetricsConfig {
    /// Master switch. Default true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Where to serve `/metrics`. `Some(addr)` → a dedicated listener on that
    /// address (default `0.0.0.0:9090`), kept off the public API surface by
    /// deployment (not published externally). `None` → mount `/metrics` on the
    /// main API listener as a sibling of `/health`.
    #[serde(default = "default_metrics_bind")]
    pub bind: Option<String>,
    /// Gauge `idle_timeout`: seconds before an un-refreshed gauge series is
    /// pruned. A silent worker's host gauges disappear after this. Default 45
    /// (≈ 3× the worker push interval; must exceed `gauge_refresh_secs`).
    #[serde(default = "default_stale_after")]
    pub stale_after_secs: u64,
    /// Seconds between server-owned gauge refresh samples. Default 5.
    #[serde(default = "default_gauge_refresh")]
    pub gauge_refresh_secs: u64,
}
```

`bind` defaults to `Some("0.0.0.0:9090")` — a separate port, network-isolated by
_not publishing_ it outside the trusted monitoring network. Authentication on
`/metrics` is **out of scope for v1** (network isolation is the control); a
future token/allowlist is noted in Risks. `validate()` must enforce two rules:
reject a `bind` equal to `listen_addr` (port clash) when both are set; and
reject `gauge_refresh_secs >= stale_after_secs`. The second is not advisory — if
the refresh cadence ever meets or exceeds the gauge idle-timeout, the
server-owned gauges idle out between refreshes (§"Risks"), so the invariant is
validated at startup rather than left to a comment.

### Endpoint wiring (`main.rs` / `app.rs`)

- `bind == Some(addr)`: build a minimal
  `Router::new().route("/metrics", get(metrics_handler)).with_state(metrics_state)`
  and `tokio::spawn` a second `axum::serve` on a listener bound to `addr`,
  sharing the existing graceful shutdown signal. Keeps metrics entirely off the
  API router.
- `bind == None`: add `.route("/metrics", get(metrics_handler))` to the main
  router beside `/health` in `build_router`.

`metrics_handler` returns `handle.render()` as `text/plain; version=0.0.4`.

### Advertising the capability (`accepts_metrics`)

The worker stays silent until the server advertises that it consumes
`WorkerMessage::Metrics` (021 §"`Welcome.accepts_metrics`"; proposal 002 N1).
021 defines the wire field and the worker's _read_; **this document owns the
write**. The server-side rule:

- When building each `ServerMessage::Welcome` (the WS handler's accept path,
  `ws/handler.rs`), set `accepts_metrics = metrics.enabled`. With metrics
  enabled (the default) the field is `true` and metrics-capable workers begin
  pushing; with `metrics.enabled == false` it is `false` and no worker pushes,
  even though the field exists on the wire.
- The flag tracks ingestion capability, not the exposition bind. A server with
  `bind == None` (metrics on the main listener) still sets `accepts_metrics`
  from `enabled` — the worker push path is independent of where `/metrics` is
  served.

This is the producer half of the handshake whose consumer is specified in 021;
without this write the field defaults to `false`, no worker ever pushes, and the
entire pushed-worker half of the design lies dormant.

## Server-owned metric registry

### Gauges — set by a periodic refresh task

A `gauge_refresh` task (default 5 s) reads `AppState.queue` and the sqlx `pool`
and sets these gauges. This **doubles as startup resync** (first tick sets
correct values, so counts survive restarts) and avoids threading gauge updates
through every transition site. The same task also calls `handle.run_upkeep()`
each tick so idle worker gauges are pruned independent of scrape cadence (§"One
facade, idle-expiring worker gauges").

| Metric                     | Labels                      | Source                           |
| -------------------------- | --------------------------- | -------------------------------- |
| `cbsd_builds_queued`       | `priority`, `arch`          | queue lanes (`queue/mod.rs`)     |
| `cbsd_builds_active`       | `arch`                      | `queue.active`                   |
| `cbsd_workers_connected`   | `state`, `arch`             | `WorkerState` (`ws/liveness.rs`) |
| `cbsd_db_pool_connections` | `state` (`acquired`/`idle`) | `pool.size()`, `pool.num_idle()` |

### Counters — emitted inline at the event

| Metric                             | Labels                                                          | Emit site                                                                         |
| ---------------------------------- | --------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `cbsd_build_results_total`         | `result`(success/failure/revoked), `arch`, `periodic`, `worker` | `build_finished` handler → `set_build_finished` (`ws/handler.rs`, `db/builds.rs`) |
| `cbsd_build_requeues_total`        | `reason`(worker_dead/ack_timeout/disconnect)                    | `rollback_dispatch_to_queued` callers                                             |
| `cbsd_build_timeouts_total`        | `arch`                                                          | build-timeout path                                                                |
| `cbsd_sigkill_escalations_total`   | —                                                               | revoke/escalation path                                                            |
| `cbsd_dispatch_ack_timeouts_total` | —                                                               | dispatch ack timer                                                                |
| `cbsd_revoke_ack_timeouts_total`   | —                                                               | revoke ack timer                                                                  |
| `cbsd_worker_reconnects_total`     | `worker`                                                        | connection migration (`ws/handler.rs`)                                            |
| `cbsd_periodic_fires_total`        | `result`                                                        | scheduler (`scheduler`)                                                           |
| `cbsd_http_requests_total`         | `route`, `method`, `status`                                     | HTTP RED layer                                                                    |

### Histograms — emitted inline, buckets fixed at recorder install

| Metric                               | Labels                     | Buckets (seconds)                                                       |
| ------------------------------------ | -------------------------- | ----------------------------------------------------------------------- |
| `cbsd_build_duration_seconds`        | `result`, `arch`, `worker` | `30, 60, 120, 240, 480, 900, 1800, 2700, 3600, 5400, 7200, 10800, +Inf` |
| `cbsd_build_queue_wait_seconds`      | `priority`, `arch`         | `1, 5, 15, 30, 60, 300, 900, 1800, 3600, +Inf`                          |
| `cbsd_dispatch_latency_seconds`      | `arch`                     | `0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, +Inf`                               |
| `cbsd_periodic_schedule_lag_seconds` | —                          | `1, 5, 15, 60, 300, 900, +Inf`                                          |
| `cbsd_http_request_duration_seconds` | `route`, `method`          | `0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, +Inf`         |

The build-duration buckets are sized for the observed range — minutes to ~1–2 h
depending on ccache warmth — with headroom to 3 h and a `+Inf` catch-all. ccache
effectiveness is read off the correlated `cbsd_worker_ccache_*` series, not a
duration label.

### Correctness rules

- **Duration guard (F6).** Observe `cbsd_build_duration_seconds` only when
  `started_at` is present and `finished_at ≥ started_at`. Revoked/failed-
  before-start builds (NULL `started_at`, cleared by
  `rollback_dispatch_to_queued`, `db/builds.rs:266`) are counted in
  `cbsd_build_results_total` but contribute no duration sample.
- **Queue-wait** is `dispatched_at − queued_at`; **dispatch latency** is the
  in-`try_dispatch` work (tarball pack + send).
- **HTTP RED layer.** A `tower`/axum middleware sibling to the existing
  `TraceLayer` (`app.rs`), labeling by **matched route pattern** (not raw path,
  to bound cardinality), method, and status class.

## Pushed-worker metrics (facade emission on each push)

There is no cache. On receiving `WorkerMessage::Metrics` from a connection, the
WS handler **writes the snapshot straight into the facade**, stamping the
`worker` label server-side from the connection's `registered_worker_id` (the
worker never sends it — F8; the same stable identity as `builds.worker_id`,
`ws/dispatch.rs` / `ws/handler.rs`):

```rust
let w = registered_worker_id;             // stable across reconnects
gauge!("cbsd_worker_host_cpu_busy_ratio", "worker" => w).set(host.cpu_busy_ratio);
gauge!("cbsd_worker_host_mem_used_bytes", "worker" => w).set(host.mem_used_bytes as f64);
// … one set() per gauge field; one per filesystem with an extra `mount` label …
counter!("cbsd_worker_subprocess_exits_total", "worker" => w, "result" => "success")
    .absolute(app.subprocess_exits.success);
counter!("cbsd_worker_metrics_push_drops_total", "worker" => w)
    .absolute(app.push_drops_total);
// … disk_{read,written}_bytes_total likewise via `.absolute(..)` …
```

Cumulative fields are republished with the `metrics` facade counter handle's
**`.absolute(v)`** method (sets the counter to an absolute value; `rate()`
absorbs the reset on worker restart, detectable via the
`cbsd_worker_uptime_seconds` gauge regression). Gauges use `.set(v)`. Because
each gauge `set()` refreshes its idle timer, a silent worker's gauges are pruned
after `stale_after` (§"One facade, idle-expiring worker gauges"); its worker-
labeled counters are intentionally left as benign flat series.

Because series are keyed by the server-stamped `worker` label, a worker that
reconnects on a new `connection_id` keeps updating one continuous series — no
cache, no reconnect-race guard (F3 resolved structurally).

| Family                                              | Type    | Labels             |
| --------------------------------------------------- | ------- | ------------------ |
| `cbsd_worker_uptime_seconds`                        | gauge   | `worker`           |
| `cbsd_worker_host_cpu_busy_ratio`                   | gauge   | `worker`           |
| `cbsd_worker_host_load1`                            | gauge   | `worker`           |
| `cbsd_worker_host_mem_{total,used,available}_bytes` | gauge   | `worker`           |
| `cbsd_worker_host_swap_{total,used}_bytes`          | gauge   | `worker`           |
| `cbsd_worker_host_fs_{total,used}_bytes`            | gauge   | `worker`, `mount`  |
| `cbsd_worker_host_disk_{read,written}_bytes_total`  | counter | `worker`           |
| `cbsd_worker_ccache_{size,max}_bytes`, `_hit_ratio` | gauge   | `worker`           |
| `cbsd_worker_spool_bytes`                           | gauge   | `worker`           |
| `cbsd_worker_subprocess_exits_total`                | counter | `worker`, `result` |
| `cbsd_worker_metrics_push_drops_total`              | counter | `worker`           |

## Work order within this document's scope

1. Recorder (with `idle_timeout(GAUGE, …)`) + `MetricsState` + config + endpoint
   wiring + the gauge-refresh/`run_upkeep` task — independently testable:
   `/metrics` returns the server-owned families and idle gauges are pruned.
2. Inline counter/histogram instrumentation at the transition sites above.
3. HTTP RED middleware.
4. Worker-snapshot ingestion: the WS-handler hook that writes each
   `WorkerMessage::Metrics` into the facade (gauges via `.set`, counters via
   `.absolute`) under the server-stamped `worker` label.

Maps onto proposal 002 work items 1–3 (steps 1–3 here) and item 6 (step 4).

## Tests

- `/metrics` exposes the server-owned families with correct types after a
  refresh tick; gauges reflect a seeded queue/pool state (startup resync).
- A finished build increments `cbsd_build_results_total` (with `worker`) and
  records one duration sample; a revoked-before-start build records the result
  but **no** duration sample (F6).
- Ingesting a `WorkerMessage::Metrics` makes the worker's gauges appear in
  `render()` under the server-stamped `worker` label. Then, **without further
  pushes**, advance past `stale_after` and call `handle.run_upkeep()` (or a
  second `render()`) — the worker gauges are now absent (idle-pruned), while a
  server-owned gauge that was re-`set()` in between persists. (The test must
  drive `run_upkeep`/`render`; merely ceasing `set()` does not prune — B1.)
- A worker reconnecting (new `connection_id`, same `registered_worker_id`)
  continues one series, not two.
- `render()` output parses as valid Prometheus exposition.

## Risks / open items

- **`/metrics` auth.** v1 relies on network isolation. If `/metrics` ever needs
  to traverse an untrusted hop, add a bearer-token or IP allowlist on the
  metrics router (a follow-up; the separate-listener design makes this a
  localized change).
- **Flat counter series for decommissioned workers.** `worker`-labeled
  counters/histograms are not idle-expired (the timeout is `GAUGE`-only, so
  server-owned counters are never pruned). A removed worker leaves benign
  zero-`rate()` series. Acceptable for a bounded fleet; if workers become
  ephemeral, revisit (e.g. a periodic relabel/drop in Prometheus).
- **`idle_timeout` is per-kind, global.** Applying it to `GAUGE` expires every
  gauge that stops being updated — which is why the server-owned gauges MUST be
  re-set on the `gauge_refresh` cadence (< `stale_after`), or they would vanish
  between scrapes. The refresh task is therefore load-bearing, not just resync.
