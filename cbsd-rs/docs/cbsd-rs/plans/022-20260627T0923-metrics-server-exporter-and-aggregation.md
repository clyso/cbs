# Phase: Metrics — Server Exporter & Aggregation

**Design document:**
`cbsd-rs/docs/cbsd-rs/design/022-20260626T0943-metrics-server-exporter-and-aggregation.md`

Companion plans: `021-…-metrics-protocol-and-worker-collector.md` (proto +
worker), `023-…-metrics-deployment-and-dashboards.md` (ops).

## Progress

| #   | Commit                                                          | ~LOC | Status  |
| --- | --------------------------------------------------------------- | ---- | ------- |
| 1   | `cbsd-rs/server: add /metrics exporter and server-owned gauges` | ~450 | Pending |
| 2   | `cbsd-rs/server: instrument build outcomes and durations`       | ~350 | Pending |
| 3   | `cbsd-rs/server: instrument queue, dispatch, and scheduler`     | ~350 | Pending |
| 4   | `cbsd-rs/server: add HTTP RED metrics layer`                    | ~300 | Pending |
| 5   | `cbsd-rs/server: advertise and ingest pushed worker metrics`    | ~400 | Pending |

**Total:** ~1850 LOC, 5 commits.

This plan owns global build steps **G1, G2a, G2b, G3, G5** (see "Global build
order" below). Commit 5 here (G5) depends on the proto types from plan 021's
first commit (G4).

---

## Global build order (all three plans)

```
G1   022#1  exporter foundation + server-owned gauges    (independent)
G2a  022#2  build outcome counters + duration histogram   (needs G1)
G2b  022#3  queue/dispatch/scheduler/liveness lifecycle    (needs G1)
G3   022#4  HTTP RED layer                                 (needs G1)
G4   021#1  cbsd-proto metrics wire types                  (independent)
G5   022#5  advertise accepts_metrics + ingest pushes      (needs G1, G4)
G6   021#2  worker collector + push                        (needs G4; e2e w/ G5)
G7   023#1  compose + prometheus + grafana dashboards      (needs all)
```

G2a and G2b are independent siblings after G1 (disjoint emit sites); the split
isolates the subtle F6 duration-guard test in G2a. Commits 1–4 deliver the
entire **server-owned** metric surface with zero dependency on the
worker/protocol half — that half lands in G4–G6.

---

## Commit 1 (G1): `/metrics` exporter and server-owned gauges

**Delivers:** `/metrics` serves the server-owned gauge families; idle pruning
and startup resync work. No worker code involved.

**Files:**

- `cbsd-rs/cbsd-server/Cargo.toml` (add `metrics`,
  `metrics-exporter-prometheus`)
- `cbsd-rs/cbsd-server/src/config.rs` (new `MetricsConfig` + `validate()` rules)
- `cbsd-rs/cbsd-server/src/metrics/mod.rs` (new — `install()` builds the
  recorder and returns `MetricsState`; `metrics_handler`. No histogram bucket
  constants here — each later commit adds its own `set_buckets_for_metric` call
  to `install()` when it first emits that histogram, so nothing is unused.)
- `cbsd-rs/cbsd-server/src/metrics/gauges.rs` (new — refresh task)
- `cbsd-rs/cbsd-server/src/app.rs` (hold `Option<MetricsState>` in `AppState`)
- `cbsd-rs/cbsd-server/src/routes/test_support.rs` (init the new
  `AppState.metrics` field — `None` — at both constructors, `:79` and `:87`, or
  the test build breaks)
- `cbsd-rs/cbsd-server/src/main.rs` (install recorder before any emit; spawn the
  gauge-refresh/`run_upkeep` task; wire the endpoint per `bind`)

**Steps:**

1. `MetricsConfig` (kebab-case, `#[serde(default)]` parent): `enabled` (default
   true), `bind: Option<String>` (default `Some("0.0.0.0:9090")`),
   `stale_after_secs` (45), `gauge_refresh_secs` (5). `validate()` rejects
   `bind == listen_addr` and `gauge_refresh_secs >= stale_after_secs` (design
   §Configuration).
2. Recorder install in `main.rs` before any metric is emitted:
   `PrometheusBuilder::new().idle_timeout(MetricKindMask::GAUGE, Some(stale_after)).install_recorder()?`;
   store the `PrometheusHandle` in `MetricsState`. When `enabled == false`,
   install nothing and serve no endpoint (`AppState.metrics == None`).
3. Endpoint: `bind == Some(addr)` → spawn a second `axum::serve` with a minimal
   router on `addr`, sharing the existing graceful shutdown signal;
   `bind == None` → add `/metrics` to the main router beside `/health`.
   `metrics_handler` returns `handle.render()` as `text/plain; version=0.0.4`.
4. Gauge-refresh task (interval `gauge_refresh_secs`): read `AppState.queue` and
   the sqlx `pool`; `set()` `cbsd_builds_queued{priority,arch}`,
   `cbsd_builds_active{arch}`, `cbsd_workers_connected{state,arch}`,
   `cbsd_db_pool_connections{state}` (acquired =
   `pool.size() - pool.num_idle()`, idle = `pool.num_idle()`); then call
   `handle.run_upkeep()` (drives idle pruning independent of scrape liveness).
   First tick is the startup resync.

**Tests:**

- `/metrics` exposes the server-owned gauges with correct types after one
  refresh tick from a seeded queue/pool state.
- `validate()` rejects `gauge_refresh >= stale_after` and a bind/listen clash.
- A gauge re-set every tick persists across a `run_upkeep` past `stale_after`
  (contrast with the worker-gauge prune asserted in commit 5).

---

## Commit 2 (G2a): build outcome counters and duration histogram

**Delivers:** build outcomes (success/failure/revoked), durations, timeouts, and
SIGKILL escalations graph from the build-terminal transition points. The F6
duration guard is isolated and tested here.

**Files (emit sites):**

- `cbsd-rs/cbsd-server/src/metrics/mod.rs` (add the
  `cbsd_build_duration_seconds` `set_buckets_for_metric` call to `install()`)
- `cbsd-rs/cbsd-server/src/ws/handler.rs` (`build_finished`: results counter +
  duration observe; build-timeout and revoke/escalation paths)
- `cbsd-rs/cbsd-server/src/db/builds.rs` (expose `started_at`/`finished_at`
  duration source fields; F6 guard)

**Steps:**

1. Register `cbsd_build_duration_seconds` `{result,arch,worker}` buckets via
   `Matcher::Full` in `install()` (design §Histograms).
2. Counters: `cbsd_build_results_total{result,arch,periodic,worker}` at
   `build_finished`; `cbsd_build_timeouts_total{arch}` on the build-timeout
   path; `cbsd_sigkill_escalations_total` on the revoke/escalation path.
3. Observe `cbsd_build_duration_seconds` only when `started_at` is present and
   `finished_at >= started_at` (F6 — a revoked/failed-before-start build, whose
   `started_at` was cleared by `rollback_dispatch_to_queued` at
   `db/builds.rs:266`, counts the result but records no duration sample).

**Tests:**

- A finished build increments `cbsd_build_results_total` (with `worker`) and
  records exactly one duration sample.
- A revoked-before-start build records the result but **no** duration sample
  (F6).
- `render()` output parses as valid Prometheus exposition.

---

## Commit 3 (G2b): queue, dispatch, scheduler, and liveness instrumentation

**Delivers:** queue-wait, retries, dispatch latency, ack timeouts, reconnects,
and periodic metrics graph from the surrounding lifecycle points. Independent of
G2a (disjoint emit sites); depends only on G1.

**Files (emit sites):**

- `cbsd-rs/cbsd-server/src/metrics/mod.rs` (add the `queue_wait`,
  `dispatch_latency`, `periodic_schedule_lag` `set_buckets_for_metric` calls to
  `install()`)
- `cbsd-rs/cbsd-server/src/queue/mod.rs` (queue-wait, requeues)
- `cbsd-rs/cbsd-server/src/ws/dispatch.rs` (dispatch latency)
- `cbsd-rs/cbsd-server/src/ws/liveness.rs` (ack timeouts, reconnects)
- `cbsd-rs/cbsd-server/src/scheduler/mod.rs` (periodic fires + schedule lag)

**Steps:**

1. Register `cbsd_build_queue_wait_seconds`, `cbsd_dispatch_latency_seconds`,
   `cbsd_periodic_schedule_lag_seconds` buckets via `Matcher::Full`.
2. Counters: `cbsd_build_requeues_total{reason}`,
   `cbsd_dispatch_ack_timeouts_total`, `cbsd_revoke_ack_timeouts_total`,
   `cbsd_worker_reconnects_total{worker}`, `cbsd_periodic_fires_total{result}`.
3. Histogram observes: queue-wait = `dispatched_at - queued_at`; dispatch
   latency = the in-`try_dispatch` work (tarball pack + send); schedule lag at
   the scheduler fire.

**Tests:**

- Queue-wait is recorded on dispatch; a requeue increments
  `cbsd_build_requeues_total{reason}`.
- A periodic fire increments `cbsd_periodic_fires_total` and records a
  schedule-lag sample.

---

## Commit 4 (G3): HTTP RED metrics layer

**Delivers:** API request rate, errors, and latency by matched route.

**Files:**

- `cbsd-rs/cbsd-server/src/metrics/http.rs` (new — tower/axum middleware)
- `cbsd-rs/cbsd-server/src/app.rs` (add the layer beside the existing
  `TraceLayer`)
- `cbsd-rs/cbsd-server/src/metrics/mod.rs` (register
  `cbsd_http_request_duration_seconds` buckets)

**Steps:**

1. Middleware labels by **matched route pattern** (not raw path — bounds
   cardinality), method, and status; emits `cbsd_http_requests_total`
   `{route,method,status}` and observes
   `cbsd_http_request_duration_seconds{route,method}`.
2. Apply only to the API router (not the separate metrics listener).

**Tests:**

- A request to a known route increments the counter with the route-pattern label
  and records one duration sample.

---

## Commit 5 (G5): advertise `accepts_metrics` and ingest pushed metrics

**Delivers:** the server tells capable workers it accepts metrics and exposes
their pushed host/ccache/spool/subprocess series under a stable `worker` label.
Depends on the proto types from plan 021 commit 1 (G4).

**Files:**

- `cbsd-rs/cbsd-server/src/ws/handler.rs` (set `accepts_metrics` in `Welcome`;
  `WorkerMessage::Metrics` ingestion hook)
- `cbsd-rs/cbsd-server/src/metrics/worker.rs` (new — snapshot → facade writer)

**Steps:**

1. **Advertise (M1).** When building each `ServerMessage::Welcome`, set
   `accepts_metrics = metrics.enabled` (design §"Advertising the capability").
   The flag is independent of the `/metrics` bind.
2. **Ingest.** On `WorkerMessage::Metrics` from a connection, stamp the `worker`
   label from `registered_worker_id` (stable across reconnects; never sent by
   the worker — F8) and write the snapshot straight into the facade: gauges via
   `gauge!(…).set(v)` (host cpu/load/mem/swap/fs, ccache, spool, uptime),
   cumulative fields via `counter!(…).absolute(v)` (disk IO, subprocess exits
   with `result`, push drops). No cache.

**Tests:**

- Ingesting a `Metrics` snapshot makes the worker gauges appear in `render()`
  under the server-stamped `worker` label.
- Without further pushes, advancing past `stale_after` then calling
  `run_upkeep()`/`render()` prunes the worker gauges, while a server-owned gauge
  re-set in between persists (B1 — pruning is render/upkeep-driven).
- A worker reconnecting (new `connection_id`, same `registered_worker_id`)
  continues one series, not two.

---

## Implementation notes

- Crate versions to pin at implementation: `metrics` 0.24.x,
  `metrics-exporter-prometheus` 0.18.x (`MetricKindMask` is a bitmask struct;
  `idle_timeout` takes `Option<Duration>`).
- The gauge-refresh task is **load-bearing**, not just resync: GAUGE
  idle-timeout would prune server-owned gauges between scrapes without it
  (design §Risks).
- No new sqlx queries are expected (gauges read pool stats + in-memory queue);
  if any commit adds a `query!`, regenerate `.sqlx/` and include it. </content>
