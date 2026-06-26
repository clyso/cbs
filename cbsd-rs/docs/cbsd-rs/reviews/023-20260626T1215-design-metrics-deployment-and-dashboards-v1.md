# 023 — Review: Metrics Deployment & Dashboards (v1)

## Verdict

**Conditional go.** Deployment topology (separate `/metrics` bind off the public
surface, single scrape target carrying both planes, server-host `node_exporter`,
zero worker daemons) is consistent with proposal 002 and with 022. Most
dashboard PromQL is well-formed and every metric name referenced here is defined
in 022. The blocking issue is **two panels group by a `worker` label that 022
does not define on those families** — the queries silently produce wrong/empty
results. A few smaller PromQL and config-consistency nits follow. Because 023 is
downstream of 022's catalog, the `worker`-label fix is shared and must be
coordinated with the 022 change.

## Findings

### MAJOR

**M1 — Two dashboard panels group by a `worker` label that does not exist on the
underlying family.** Verified each `cbsd_*` reference against 022's catalog.

- **Line 124, "Throughput by worker":**
  `sum by (worker) (rate(cbsd_build_results_total[15m]))`. 022 line 136 defines
  `cbsd_build_results_total{result, arch, periodic}` — **no `worker` label**.
  `sum by (worker)` over a series with no `worker` label produces a single group
  with an empty `worker=""`; the panel shows one aggregate line, not per-worker
  throughput. **Broken.**
- **Line 148, "CPU vs active builds (per worker)":**
  `sum by (worker) (cbsd_builds_active)`. 022 line 128 defines
  `cbsd_builds_active{arch}` — **no `worker` label**. Same failure. The doc
  itself hedges "(active-by-worker via plane-1 if added) or build-event
  annotations", which acknowledges the metric does not exist yet but ships the
  broken PromQL as the contract.

Fix (coordinated with 022): add a `worker` label to `cbsd_build_results_total`
and expose a per-`worker` `cbsd_builds_active` in 022, **or** rewrite these two
panels to use families that already carry `worker`. Note that
`cbsd_build_duration_seconds` _does_ carry `worker` (022 line 150), so "Duration
p95 by worker" (line 123) and "Reconnects by worker" (line 125,
`cbsd_worker_reconnects_total{worker}`, 022 line 142) are **fine** — the defect
is specific to results-total and builds-active.

### MINOR

**m1 — Build-duration histogram quantiles omit `arch`/`worker` grouping where it
likely matters, and the heatmap query is non-standard.**

- Line 113 "Duration heatmap" uses
  `rate(cbsd_build_duration_seconds_bucket[5m])` summed by `le`, producing a
  single distribution. That renders, but a Grafana heatmap from a histogram
  normally wants the per-`le` buckets _without_ collapsing — the `sum by (le)`
  is fine for a fleet-wide heatmap; just confirm the panel is intended
  fleet-wide (it loses per-arch/worker structure). Not a bug, a scoping note.
- Lines 105/111/112/123 quantiles use `sum by (le)` (and `le, worker`) — correct
  `histogram_quantile` form. No issue.

**m2 — Success-ratio panel can divide by zero / NaN at low traffic.** Line 103:
`sum(rate(...{result="success"}[1h])) / sum(rate(cbsd_build_results_total[1h]))`.
With no builds in the window the denominator is 0 → the stat shows `NaN`. Add
`clamp_min(denominator, …)` or an `or vector(0)` guard, or accept the empty
panel. Minor, but it is a flagship SLO tile.

**m3 — `cbsd_db_pool_connections{state="acquired"}` vs literal `4`.** Line 161
compares against a hard-coded `4`. This mirrors CLAUDE invariant #2 (the
`max_connections = 4` ceiling) and is a deliberate "make the deadlock ceiling
visible" panel (a genuine strength). But the `4` is a magic literal that drifts
if the pool size changes; note that the threshold should track the config, or
annotate the panel with the source-of-truth.

**m4 — node_exporter port/job is internally consistent; double-check the
`node-server` job name vs dashboard 1860.** Lines 70-72 scrape
`node-exporter:9100` under `job_name: node-server`; line 174 references the
community 1860 board. Board 1860 templates on `job` and `instance` label values
— fine, just ensure the provisioned datasource/board variables match the
`node-server` job label so the imported dashboard populates. Operational note,
not a defect.

**m5 — Compose port note is terse but correct.** Line 50 "publish 9091→9090 for
local debugging" (Prometheus UI) plus line 54-56 "cbsd-server `metrics.bind`
(9090) exposed only on the compose network" are consistent: the server listens
on 9090 inside the network (reached as `server-dev:9090`, line 69), and
Prometheus's own UI is published host-9091→container-9090. No clash because they
are different containers. Worth a one-line clarification that the two `9090`s
are on different containers, to preempt reader confusion.

### OBSERVATIONS / NON-ISSUES (verified)

- Config examples (lines 21-38) use kebab-case keys (`stale-after-secs`,
  `gauge-refresh-secs`, `push-interval-secs`, `ccache-interval-secs`) matching
  022/021 `#[serde(rename_all = "kebab-case")]`. `bind: "0.0.0.0:9090"` and the
  `null` ⇒ main-listener note (line 24-25) align with 022's `Option<String>`
  bind semantics. Consistent.
- Single scrape target carrying both planes (lines 75-78) is correct given 022
  concatenates plane-1 `handle.render()` and plane-2 `workers.render()` on one
  `/metrics`. No per-worker discovery needed — matches proposal 002.
- The `worker` template variable
  `label_values(cbsd_worker_uptime_seconds, worker)` (line 153) is valid: 022
  defines `cbsd_worker_uptime_seconds{worker}` (line 220). Good choice of a
  per-worker plane-2 series for discovery.
- "No worker-side services" (line 45) upholds the proposal's defining
  constraint.
- All other metric names used (`cbsd_builds_queued{priority}`,
  `cbsd_build_requeues_total{reason}`, `cbsd_workers_connected{state}`,
  `cbsd_http_request*`, `cbsd_periodic_fires_total{result}`, the
  `cbsd_worker_host_*`/`ccache`/`spool`/`push_drops` families) resolve to 022
  definitions with matching labels. No orphan dashboard references except M1.

## Cross-document consistency

This is the document where cross-doc drift surfaces, since 023 consumes 022's
catalog. Summary:

1. **`worker`-label mismatch (M1) — MAJOR.** `cbsd_build_results_total` and
   `cbsd_builds_active` are grouped `by (worker)` here but defined without a
   `worker` label in 022 (lines 136, 128). Root-cause and fix live in 022; 023's
   panels must change in lockstep.
2. **`outcome` vs `result` label drift.** 023 dashboards use `result` throughout
   (`{result="success"}`, `by (result)`), which matches 022's
   `cbsd_build_results_total{result}` — but 022/021 label the worker subprocess
   counter `cbsd_worker_subprocess_exits_total{outcome}`. 023 does **not**
   currently chart subprocess exits by outcome, so no broken query here, but if
   such a panel is added it must use `outcome`, not `result`. Standardizing the
   label (recommend `result`) in 021/022 would remove the trap.
3. **Names/units — consistent.** All dashboard references use the 022 family
   names verbatim, `_bucket` suffixes on histogram quantiles are correct, and
   units (`_seconds`, `_bytes`, ratios) line up across all three documents.

## Confidence score

| Item                                                               | Points | Description                                                              |
| ------------------------------------------------------------------ | ------ | ------------------------------------------------------------------------ |
| Starting score                                                     | 100    |                                                                          |
| D8: M1 "Throughput by worker" groups a non-existent `worker` label | -5     | `cbsd_build_results_total` has no `worker` label (022:136); panel broken |
| D8: M1 "CPU vs active builds" groups a non-existent `worker` label | -5     | `cbsd_builds_active` has no `worker` label (022:128); panel broken       |
| D8: m2 success-ratio divides by zero at low traffic                | -5     | Flagship SLO tile shows `NaN` with no builds in window                   |
| D11: m3 hard-coded `4` pool ceiling drifts from config             | -5     | Magic literal not tracked to source-of-truth                             |
| **Total**                                                          | **80** |                                                                          |

**Interpretation: 80 / 100 — acceptable with noted improvements; fix before the
next stage.** 023 is the strongest of the three: the topology is right and the
PromQL is mostly correct. The two `worker`-label panels are the only real
defects, and their fix is shared with 022.

### Most important fix

Resolve the `worker`-label mismatch (M1): coordinate with 022 to add a `worker`
label to `cbsd_build_results_total` and a per-`worker` `cbsd_builds_active`, or
rewrite the "Throughput by worker" (line 124) and "CPU vs active builds"
(line 148) panels to use families that already carry `worker`
(`cbsd_build_duration_seconds`, `cbsd_worker_reconnects_total`).
