# 022 — Review: Metrics Server Exporter & Aggregation (v2)

## Verdict

**Conditional go.** The round-1 majors are both resolved, and the resolution is
the right one: the false "the recorder cannot expire individual labeled series"
premise is gone, the custom `WorkerMetricsCache` is deleted, and the design now
routes everything through one `metrics-exporter-prometheus` recorder using
`idle_timeout(MetricKindMask::GAUGE, …)` — which I verified on docs.rs does
exactly what the doc claims. The `worker`-label gap on
`cbsd_build_results_total` is fixed. However, the rewrite introduces **one new
blocking gap**: the crate's idle expiry is **render-driven** (pruning only
occurs when `render()` or `run_upkeep()` runs — there is no background timer),
and the doc never establishes what drives `render()` between Prometheus scrapes.
The staleness guarantee — and the test that asserts it — silently depends on
this, and the "refresh task driving render" phrasing in the test is inconsistent
with the refresh task as designed (which drives `set()`, not `render()`). This
is a load-bearing omission, not a deep flaw: a one-line `run_upkeep()` timer (or
an explicit "scrape drives pruning" invariant with a stated bound) closes it.

Reviewed against source on `wip/cbsd-rs-metrics` and current docs.rs stable
(`metrics-exporter-prometheus` 0.18.3, `metrics` 0.24.6, `metrics-util` 0.20.4,
`sqlx` per workspace pin).

## Disposition of round-1 findings

- **M1 — false "facade recorder cannot expire individual labeled series" premise
  — RESOLVED.** The custom `WorkerMetricsCache` is gone entirely. The new "One
  facade, idle-expiring worker gauges" section (lines 17-52) states the truth
  and even calls out the prior error: "An earlier draft used a custom
  `WorkerMetricsCache` on the belief that the recorder could not expire
  individual labeled series; it can." Verified on docs.rs:
  `PrometheusBuilder::idle_timeout(self, mask: MetricKindMask, timeout: Option<Duration>) -> Self`
  exists and prunes per-series (name + label set) on last-update time. The
  GAUGE-only decision (lines 45-52) is sound: silent worker gauges expire;
  server-owned counters are never pruned. This is a clean, well-reasoned rewrite
  — the strongest improvement in the set.

- **M2 — `cbsd_build_results_total` missing the `worker` label that 023 groups
  by — RESOLVED.** Line 152 now defines
  `cbsd_build_results_total{result, arch, periodic, worker}`. 023's "Throughput
  by worker" (`sum by (worker) (rate(cbsd_build_results_total[15m]))`, line 125)
  and the rewritten correlation panel (lines 150-151) now resolve against a real
  label. The other half of v1 M2 — `cbsd_builds_active` lacked `worker` — is
  resolved **structurally**: 023's correlation panel no longer groups
  `cbsd_builds_active` by worker; it was rewritten to overlay
  `cbsd_build_results_total{worker}` instead, and `cbsd_builds_active` is now
  only ever used as `sum(cbsd_builds_active)` (023 line 102). So
  `cbsd_builds_active{arch}` (line 144, no `worker`) is now internally
  consistent with every 023 query that touches it. No orphan groupings remain.

- **Crate-API notation (`.absolute(v)`, `Matcher::Full`, `?` on builder calls) —
  RESOLVED.** Line 204-207 use `.absolute(v)` (instance method, confirmed:
  `Counter::absolute(&self, value: u64)`). The recorder snippet (lines 61-68)
  now `?`s `set_buckets_for_metric` and `install_recorder` (both confirmed to
  return `Result<_, BuildError>`), and `Matcher::Full(String)` is a real
  variant. Gauge `.set(v)` is valid (`Gauge::set<T: IntoF64>`; `f64: IntoF64`).

- **`outcome` → `result` drift — RESOLVED.**
  `cbsd_worker_subprocess_exits_total` now carries `{worker, result}` (line
  233), matching `cbsd_build_results_total {result}`. No `outcome` label
  remains.

## New findings

### BLOCKING

**B1 — idle expiry is render-driven; the doc never establishes what drives
`render()`/`run_upkeep()` between scrapes, and the staleness test is
inconsistent with the refresh task.** This is the one place the big rewrite
under-specifies a load-bearing mechanism. Verified verbatim on docs.rs
(`metrics-exporter-prometheus` 0.18.3,
https://docs.rs/metrics-exporter-prometheus/latest/metrics_exporter_prometheus/struct.PrometheusBuilder.html):

> "If a metric hasn't been updated within this timeout, it will be removed from
> the registry … **This behavior is driven by requests to generate rendered
> output, and so metrics will not be removed unless a request has been made
> recently enough to prune the idle metrics.**"

`PrometheusHandle::run_upkeep(&self)` exists precisely to drive this without a
scrape. Consequences the design does not address:

1. **Pruning is not time-based; it is render-based.** A silent worker's gauges
   disappear only when `render()` (a `/metrics` scrape) or `run_upkeep()` next
   runs. If Prometheus pauses scraping or scrapes slower than `stale_after`, the
   stale gauges persist past `stale_after` — the exact "frozen CPU gauge"
   outcome the section claims to prevent. The doc's own scrape cadence (023:
   `scrape_interval: 15s`, `stale_after = 45s`) makes this fine in the happy
   path, but the design states the guarantee unconditionally and never names the
   dependency on scrape liveness. It should either (a) add a `run_upkeep()`
   timer so pruning is independent of scrape liveness, or (b) state explicitly
   that staleness pruning is bounded by the scrape interval and breaks if
   scraping stops.

2. **The staleness test is wrong as written.** Line 257-258: "after no further
   pushes for > `stale_after` (with the refresh task driving `render`), those
   gauge series are absent." But the `gauge_refresh` task as designed (lines
   136-146, 279) drives `gauge.set()` on the server-owned gauges — it does
   **not** call `handle.render()`. Nothing in the design calls `render()` on a
   timer. So in the test environment (no live Prometheus scraping), the worker
   gauges will **not** be pruned unless the test itself calls `render()` (or
   `run_upkeep()`) after `stale_after`. The parenthetical "with the refresh task
   driving render" describes a mechanism the design does not implement. Fix the
   test to call `render()`/`run_upkeep()` explicitly after the idle window, and
   fix the prose to match.

   Note the risk #3 bullet (lines 276-279) correctly identifies that the refresh
   task is load-bearing for keeping **server-owned gauges alive** — but it
   conflates that with driving pruning of **worker gauges**. Keeping a gauge
   alive (`set()` resets its idle timer) and pruning an idle gauge (`render`/
   `run_upkeep` sweeps it) are two different mechanisms; the doc treats the
   refresh task as doing both. It only does the former.

   **Fix:** the cleanest resolution is a single periodic task that calls
   `handle.run_upkeep()` (and/or the gauge refresh) on a cadence
   `< stale_after`, making pruning independent of scrape liveness; then state
   that invariant once and align the test to it.

### MINOR

**m1 — DB-pool gauge: `acquired` is derived, not an accessor.** Line 146 sources
`cbsd_db_pool_connections{state}` from `pool.size()` / `pool.num_idle()`.
Confirmed both exist (sqlx `Pool::size() -> u32`, `Pool::num_idle() -> usize`),
but `acquired = size() - num_idle()` is a derived value — there is no
`acquired()` accessor. State the derivation so the implementer doesn't hunt for
one. (Carried from v1 m2; still unstated.)

**m2 — `validate()` bind-clash check is a string compare.** Line 118 rejects
`bind == listen_addr`. Equivalent-but-differently-spelled addresses (`0.0.0.0`
vs `[::]`, hostname vs literal, same port) bypass it. Parse both to `SocketAddr`
and compare port/wildcard rather than raw strings. (Carried from v1 m4.)

**m3 — first `gauge_refresh` tick lag emits empty server-owned gauges for up to
`gauge_refresh_secs`.** Lines 136-140 lean on the periodic task for startup
resync; the first tick fires up to 5 s post-restart, so a scrape in that window
shows absent/zero queue/pool gauges. The doc now frames the task as "doubles as
startup resync" (good) but doesn't note the first-tick window. Optionally run
one refresh synchronously before serving. (Carried from v1 m3.) Note this
interacts with B1: if a `run_upkeep`/refresh task is added, run its first tick
before the listener accepts scrapes.

**m4 — `MetricKindMask` is a struct with bitmask consts, not an enum.**
Cosmetic: `metrics-util` 0.20.4 exposes `MetricKindMask` as a struct with
associated consts (`NONE`/`COUNTER`/`GAUGE`/`HISTOGRAM`/`ALL`) supporting
bitwise OR, not a Rust `enum`. `MetricKindMask::GAUGE` is correct as written;
just don't describe it as an enum in any future prose (the doc doesn't currently
mislabel it — noted for the implementer who may look up the wrong `enum.` docs
URL).

### OBSERVATIONS / NON-ISSUES (re-verified)

- `AppState { pool, queue }` — confirmed `cbsd-server/src/app.rs:54-59`. Adding
  `metrics: Option<MetricsState>` is consistent.
- Single `axum::serve(listener, …).with_graceful_shutdown(…)` on
  `config.listen_addr` — confirmed `cbsd-server/src/main.rs:289-293`. The second
  `tokio::spawn`ed `axum::serve` for `/metrics` sharing the shutdown signal
  (lines 122-128) is the right shape.
- F6 duration guard — confirmed: `rollback_dispatch_to_queued` sets
  `started_at = NULL` (`db/builds.rs:266`), `set_build_finished` stamps
  `finished_at = unixepoch()` (`db/builds.rs:342`). Observing duration only when
  `started_at` present and `finished_at >= started_at` (lines 180-183) is
  correct.
- `install_recorder() -> Result<PrometheusHandle, BuildError>` and
  `render(&self) -> String` — confirmed.
- Server-stamping `worker` from `registered_worker_id` (lines 198-200, F8) —
  sound; matches `builds.worker_id` identity, survives reconnects, dissolves the
  F3 reconnect-race structurally (series keyed by `worker` label, not
  `connection_id`).
- **GAUGE-only idle-timeout quantitative check — holds.** I verified there is no
  server-owned or worker GAUGE updated on a cadence slower than `stale_after`
  (45 s) that would be wrongly pruned: server gauges are re-`set()` every
  `gauge_refresh` (5 s ≪ 45 s); worker host/ccache/spool/uptime gauges are
  re-`set()` on every push (15 s < 45 s). The ccache value is sampled on a
  slower 60 s cadence but is **carried forward** and re-pushed every 15 s (021
  lines 97-99, 244), so its gauge's idle timer is reset on each 15 s push
  despite the 60 s sampling — confirmed it is NOT pruned. The reasoning the task
  asked me to check holds. (This correctness is, however, contingent on B1: the
  pushes reset the timer, but a render/upkeep must still run to sweep a
  genuinely-silent worker.)

## Cross-document consistency

- **Plane catalog ↔ 021 schema — clean (re-confirmed post-edit).** Every 022
  family maps to a 021 schema field and vice-versa; no orphans either direction.
- **022 ↔ 023 label sets — clean.** Every `by (worker)` grouping in 023 now hits
  a family that carries `worker`: `cbsd_build_results_total` (now labeled, line
  152), `cbsd_build_duration_seconds` (line 166), `cbsd_worker_reconnects_total`
  (line 158), `cbsd_worker_host_*` (lines 225-229). `cbsd_builds_active{arch}`
  (no `worker`) is only consumed as `sum(cbsd_builds_active)` in 023 — no
  mismatch. The v1 M2 cross-doc defect is fully closed.
- **Label vocabulary — consistent.** `result` for success/failure/revoked
  everywhere; `_total`/`_seconds`/`_bytes` suffixes correct across the catalog.

## Confidence score

| Item                                                              | Points | Description                                                                                                     |
| ----------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------- |
| Starting score                                                    | 100    |                                                                                                                 |
| D8: B1 render-driven idle expiry; no `render`/`run_upkeep` driver | -15    | Staleness guarantee depends on an unstated scrape-liveness/upkeep mechanism (D8 ×2-ish, scored as one blocking) |
| D5: B1 staleness test asserts a mechanism the design lacks        | -10    | Test "refresh task driving render" is inconsistent; would not prune without explicit `render()`/`run_upkeep()`  |
| D3: m1 `acquired` derived, not an API accessor                    | -5     | Data-source detail under-specified                                                                              |
| D7: m2 bind-clash check is string compare, not `SocketAddr`       | -5     | Equivalent addresses bypass the port-clash guard                                                                |
| D9: m3 first-tick gauge-resync window emits empty gauges          | -5     | Observable wrong values for up to `gauge_refresh_secs` post-restart                                             |
| **Total**                                                         | **60** |                                                                                                                 |

**Interpretation: 60 / 100 — significant issues; address before proceeding.**
**Delta vs v1: −10 (70 → 60).** This is a deliberate, defensible drop: v1's
−5/−5 for the two majors (false-premise rationale and the worker-label gap) are
both fully recovered (+10), but the rewrite that fixed them introduced a heavier
blocking gap (B1, −25 combined) by adopting `idle_timeout` without specifying
the render/upkeep driver it depends on. The architecture is now correct and
simpler than v1; the score reflects that the single remaining defect is
load-bearing (staleness can silently fail and the test would pass spuriously).
Closing B1 lifts this well into the 85+ band — the carry-over minors are all
trivial.

### Most important remaining fix

Add an explicit periodic `handle.run_upkeep()` driver (cadence < `stale_after`,
first tick before the listener serves), state once that idle-gauge pruning is
driven by render/upkeep (not the gauge-`set` refresh task), and correct the
staleness test (lines 256-260) to call `render()`/`run_upkeep()` after the idle
window rather than relying on "the refresh task driving render."
