# 022 — Review: Metrics Server Exporter & Aggregation (v1)

## Verdict

**Conditional go.** The server-side design is coherent and the codebase anchors
all check out (`AppState` shape, single `axum::serve`, pool introspection APIs,
the F6 duration guard, `set_build_finished` stamping). The **central
justification for the custom `WorkerMetricsCache` is factually overstated**:
`metrics-exporter-prometheus` _does_ expose a per-series idle-expiry mechanism
(`idle_timeout` + `MetricKindMask`), including for gauges — so the claim that
the recorder "cannot expire individual labeled series" is false as written. The
custom cache may still be the right call (instant vs idle-based removal,
absolute-counter republish), but the rationale must be re-grounded. Several
crate-API notations need correction. A real **major** cross-doc defect:
dashboards in 023 group by a `worker` label that two plane-1 families here do
not carry.

Reviewed against source on `wip/cbsd-rs-metrics` and current docs.rs stable.

## Findings

### MAJOR

**M1 — "the facade recorder cannot expire individual labeled series" is false.**
Lines 26-29 and 36-41 build the two-plane split on the premise that
`metrics-exporter-prometheus` cannot drop a stale gauge series, so a custom
freshness-gated renderer is _required_. The crate **does** provide this:

```rust
PrometheusBuilder::new()
    .idle_timeout(MetricKindMask::GAUGE | MetricKindMask::ALL, Some(dur))
```

`PrometheusBuilder::idle_timeout(mask: MetricKindMask, timeout: Option<Duration>)`
(https://docs.rs/metrics-exporter-prometheus/latest/metrics_exporter_prometheus/struct.PrometheusBuilder.html)
prunes **per-metric-key** series — where a key is name **plus its label set** —
once that specific series has not been updated within the timeout.
`MetricKindMask` (`metrics-util`) includes `GAUGE`, so gauges are covered;
recency is tracked in `metrics_util::registry::Recency`, and pruning is
reflected in `render()` after upkeep. Therefore a decommissioned worker's gauge
would **not** "show a frozen CPU gauge forever" — it would expire.

This does not necessarily kill the custom cache, but it changes its
justification. The defensible residual reasons to still build it:

1. **Absolute counter republish.** Plane 2 republishes cumulative counters at
   pushed absolute values (`subprocess_exits`, `disk_*_bytes_total`,
   `push_drops_total`). The facade's `counter!(...).absolute(v)` _can_ do this,
   but feeding pushed snapshots through the global recorder mixes server-owned
   and worker-pushed series in one registry and complicates per-worker
   `last_push` freshness logic.
2. **Deterministic/instant disappearance.** `idle_timeout` only removes a series
   _after_ it has been idle for the window — it cannot remove on demand. If the
   requirement is "gone the instant the worker is `Dead`", the optional eviction
   in lines 202-207 is what delivers that, not the freshness window alone.

Fix: rewrite lines 26-41 to state the recorder _can_ idle-expire series (cite
`idle_timeout`/`MetricKindMask`), and re-justify the custom cache on the actual
grounds (push-driven absolute counters + server-stamped `worker` label +
explicit freshness/eviction control), or seriously evaluate routing plane-2
through the facade with `idle_timeout` and dropping the custom renderer. As
written, a reviewer who knows the crate will reject the stated premise.

**M2 — `cbsd_build_results_total` and `cbsd_builds_active` lack the `worker`
label that 023 groups by (cross-doc).** See "Cross-document consistency" — a
defect that lives in this doc's label catalog. Two 023 dashboard panels are
broken against the metrics as defined here.

### MINOR

**m1 — Crate-API notation/signature corrections (version-sensitive).** The
crates are unpinned; current stable differs from the snippet's implied use:

- `set_buckets_for_metric(Matcher::Full(...), &[f64])` returns
  `Result<Self, BuildError>` — the chain in lines 50-54 must `?`/unwrap each
  call. `Matcher::Full` is correct (variants: `Full`, `Prefix`, `Suffix`).
- `install_recorder()` returns `Result<PrometheusHandle, BuildError>` (line 54
  comment "global recorder; returns the render handle" omits the `Result`).
- `PrometheusHandle::render() -> String` — correct.
- `Counter::absolute()` (line 214) — `absolute` is an **instance method**
  (`counter!("name", labels).absolute(v)`), not an associated fn. Same fix as
  021/m1.
- Note current stable is `metrics-exporter-prometheus` 0.18.x and `metrics`
  0.24.x — pin them; the API above is from those versions.

(https://docs.rs/metrics-exporter-prometheus/latest/,
https://docs.rs/metrics/latest/metrics/struct.Counter.html)

**m2 — DB-pool gauge: confirm `num_idle()` return type for label math.**
`Pool::size() -> u32` and `Pool::num_idle() -> usize` both exist in sqlx (latest
stable 0.9.x; CLAUDE pins ~0.8) —
https://docs.rs/sqlx/latest/sqlx/struct.Pool.html. The
`cbsd_db_pool_connections{state}` design (line 130) is feasible. Note
`acquired = size() - num_idle()` is a derived value, not a direct API; state
that derivation explicitly so the implementer does not look for an `acquired()`
accessor.

**m3 — `gauge_refresh` task as "startup resync" is sound but racy on the first
tick.** Lines 119-124 lean on the periodic task to also seed gauges after
restart. The first tick fires up to `gauge_refresh_secs` (default 5 s) after
startup, so for up to 5 s post-restart `/metrics` reports zero/absent gauge
values. Acceptable, but call it out (a scrape landing in that window shows a
spurious empty queue). Optionally run one refresh synchronously before serving.

**m4 — `validate()` bind-clash check is necessary but incomplete.** Line 101
rejects `bind == listen_addr`. Good. But the default `bind` is `0.0.0.0:9090`
and `listen_addr` could be `0.0.0.0:9090`-equivalent expressed differently (e.g.
`[::]:9090`, hostname vs `0.0.0.0`); a string compare misses these. Parse both
to `SocketAddr` and compare port (and overlapping wildcard) rather than raw
strings.

### OBSERVATIONS / NON-ISSUES (verified)

- `AppState { pool, queue, … }` with `#[derive(Clone)]` (Arc-wrapped interiors)
  — **confirmed** `cbsd-server/src/app.rs:53-75`. Adding
  `metrics: Option<MetricsState>` is consistent with how `AppState` is
  built/cloned.
- Single `axum::serve(listener, …).with_graceful_shutdown(…)` on
  `config.listen_addr` — **confirmed** `cbsd-server/src/main.rs:289`. A second
  `tokio::spawn`ed `axum::serve` on a separate listener for `/metrics` (lines
  105-109) is the right shape and shares the existing shutdown signal.
- F6 duration guard premise — **confirmed**: `rollback_dispatch_to_queued` sets
  `started_at = NULL` (`cbsd-server/src/db/builds.rs:266`) and
  `set_build_finished` stamps `finished_at = unixepoch()` (`db/builds.rs:342`).
  Observing duration only when `started_at` present and
  `finished_at >= started_at` (lines 163-167) is correct; the doc's
  `db/builds.rs:266` citation is accurate.
- Config pattern (`#[serde(rename_all = "kebab-case")]`, `#[serde(default)]`
  section, `serde_saphyr`) — **confirmed** `cbsd-server/src/config.rs`. Caveat
  matching 021: **no `default_true` helper exists today**; the design's
  `default_true`/`default_metrics_bind`/`default_stale_after`/
  `default_gauge_refresh` are net-new helpers (trivial).
- `Option<String> bind` with `#[serde(default = "default_metrics_bind")]`
  returning `Some("0.0.0.0:9090")` parses correctly under serde; YAML `null` ⇒
  `None` ⇒ mount on main router. Behaviour is well-defined. (The codebase's
  existing `Option<PathBuf>` fields use no default fn, but a default fn on an
  `Option` is valid serde and parses as intended.)
- Server-stamping the `worker` label from `registered_worker_id` (lines 184-189,
  F8) — sound; matches `builds.worker_id` identity and survives reconnects (not
  keyed on `connection_id`).

## Cross-document consistency

- **M2 / dashboard label mismatch (with 023) — MAJOR.** Two 023 panels group by
  a `worker` label this catalog does not define:
  - 023 line 124 "Throughput by worker":
    `sum by (worker) (rate(cbsd_build_results_total[15m]))`, but here
    `cbsd_build_results_total` has labels `result, arch, periodic` (line 136) —
    **no `worker`**. The query collapses to a single group; the panel is
    meaningless.
  - 023 line 148 "CPU vs active builds": `sum by (worker) (cbsd_builds_active)`,
    but here `cbsd_builds_active` has label `arch` only (line 128) — **no
    `worker`**. 023 hedges this in prose ("active-by-worker via plane-1 if
    added"), confirming the gap is known but unresolved.

  Resolution belongs in **this** document: either add a `worker` label to
  `cbsd_build_results_total` and a per-`worker` `cbsd_builds_active` (the
  build-finished and active-count sites both know the worker), or change 023 to
  drop those `by (worker)` groupings. The build-duration histogram already
  carries `worker` (line 150), so per-worker throughput/active is the natural
  parallel — adding the label here is the cleaner fix. Note the cardinality cost
  is bounded by fleet size and acceptable.

- **Label-name drift `outcome` vs `result`.**
  `cbsd_worker_subprocess_exits_total {outcome}` (line 229) vs
  `cbsd_build_results_total{result}` (line 136) label the same
  success/failure/revoked classification differently. Standardize (recommend
  `result` everywhere). Also flagged in 021.

- **Plane-2 catalog ↔ 021 schema — clean.** Every family here maps to a 021
  schema field and vice-versa; no orphans (cross-checked in 021's review).

- **Units/suffix conventions — consistent.** `_seconds` on durations, `_bytes`
  on sizes, `_total` on counters, ratios unit-less. No violations found.

## Confidence score

| Item                                                                 | Points | Description                                                                        |
| -------------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------- |
| Starting score                                                       | 100    |                                                                                    |
| D8: M1 false "recorder cannot expire series" premise                 | -5     | `idle_timeout`/`MetricKindMask` does per-series gauge expiry; core rationale wrong |
| D8: M2 `cbsd_build_results_total`/`cbsd_builds_active` miss `worker` | -5     | Label catalog inconsistent with 023 dashboards (cross-doc)                         |
| D8: m1 crate-API notation/`Result`/`Counter::absolute` errors        | -5     | Several version-sensitive API inaccuracies in snippets                             |
| D3: m2 `acquired` derived, not an API accessor                       | -5     | Data-source detail under-specified                                                 |
| D9: m3 first-tick gauge-resync window emits empty gauges             | -5     | Observable wrong values for up to `gauge_refresh_secs` post-restart                |
| D7: m4 bind-clash check is string-compare, not `SocketAddr`          | -5     | Validation gap: equivalent addresses bypass the port-clash guard                   |
| **Total**                                                            | **70** |                                                                                    |

**Interpretation: 70 / 100 — significant issues; address before proceeding.** M1
(premise) and M2 (label mismatch) are the load-bearing fixes; the rest are minor
corrections. The structural server design is otherwise correct and
codebase-faithful.

### Most important fix

Re-ground the two-plane rationale (lines 26-41): acknowledge that
`metrics-exporter-prometheus` _can_ idle-expire individual labeled gauge series
via `idle_timeout(MetricKindMask, Some(dur))`, then either (a) re-justify the
custom `WorkerMetricsCache` on the real grounds (pushed absolute counters +
server-stamped identity + instant `Dead` eviction), or (b) adopt the facade's
idle timeout and drop the custom renderer.
