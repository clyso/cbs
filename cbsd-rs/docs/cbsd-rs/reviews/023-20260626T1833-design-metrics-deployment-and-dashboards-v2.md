# 023 — Review: Metrics Deployment & Dashboards (v2)

## Verdict

**Go.** The round-1 major — two panels grouping by a `worker` label that 022 did
not define — is fully resolved, partly by 022 adding the label and partly by
rewriting the correlation panel so it no longer groups `cbsd_builds_active` by
worker. Every `cbsd_*` reference in every panel now resolves to a 022 family
with the exact label set the PromQL uses (re-verified family-by-family).
Deployment topology (separate `/metrics` bind off the public surface, single
scrape target carrying both server-owned and pushed-worker series, server-host
node_exporter, zero worker daemons) remains consistent with proposal 002
and 022. Two pre- existing minors carry over (the success-ratio NaN guard and
the hard-coded pool `4`); neither blocks. No new defects from the revision.

Reviewed against 022's v2 catalog and source on `wip/cbsd-rs-metrics`.

## Disposition of round-1 findings

- **M1 — two panels group by a non-existent `worker` label — RESOLVED.**
  - "Throughput by worker" (line 125):
    `sum by (worker) (rate(cbsd_build_results_total[15m]))`. 022 now defines
    `cbsd_build_results_total{result, arch, periodic, worker}` (022 line 152),
    so the grouping is valid and produces real per-worker series. Fixed.
  - The old "CPU vs active builds (per worker)" panel that grouped
    `cbsd_builds_active` by `worker` (which 022 still defines with `{arch}`
    only, line 144) is **gone**. The correlation dashboard (lines 148-152) was
    rewritten to overlay `cbsd_worker_host_cpu_busy_ratio{worker="$worker"}`
    with
    `sum by (worker) (rate(cbsd_build_results_total{worker="$worker"}[5m]))` —
    both of which carry `worker`. `cbsd_builds_active` is now used only as
    `sum(cbsd_builds_active)` (line 102), which needs no `worker` label. The
    mismatch is closed from both ends.

- **`outcome` vs `result` label drift — RESOLVED.** 023 uses `result` throughout
  (`{result="success"}`, `by (result)`, the "Periodic outcomes" panel charts
  `sum by (result) (rate(cbsd_periodic_fires_total[1d]))`), matching 022's
  `cbsd_build_results_total{result}` and
  `cbsd_worker_subprocess_exits_total {result}`. The v1 trap ("a future
  subprocess-exit-by-outcome panel would have to use `outcome`") is gone because
  022/021 renamed the label to `result`. The sole remaining `outcome` token is
  the prose panel title "Periodic outcomes," which correctly uses the `result`
  label underneath.

## New findings

### MINOR (carried from v1, still open)

**m1 — success-ratio panel divides by zero / NaN at low traffic.** Line 104:
`sum(rate(...{result="success"}[1h])) / sum(rate(cbsd_build_results_total[1h]))`.
With no builds in the window the denominator is 0 → the flagship SLO stat shows
`NaN`. Guard with `clamp_min(...)` / `or vector(0)`, or accept the empty tile.
Low impact but it is a headline panel.

**m2 — `cbsd_db_pool_connections{state="acquired"}` compared to a literal `4`.**
Line 163 hard-codes the `max_connections = 4` ceiling (CLAUDE.md invariant #2).
Making the deadlock ceiling visible is a genuine strength, but the `4` drifts if
the pool size changes. Track it to the config source-of-truth or annotate the
panel. Low impact.

### OBSERVATIONS / NON-ISSUES (re-verified)

- **Every panel metric resolves to a 022 family with the right label set.** I
  re-checked all `cbsd_*` references: queue/throughput/duration/per-worker/host-
  ccache/correlation/fleet panels all hit defined families. The `by (worker)`
  groupings now all land on labeled families (`cbsd_build_results_total`,
  `cbsd_build_duration_seconds`, `cbsd_worker_reconnects_total`,
  `cbsd_worker_host_*`). The `worker` template variable
  `label_values(cbsd_worker_uptime_seconds, worker)` (line 155) is valid — 022
  defines `cbsd_worker_uptime_seconds{worker}` (line 224).
- Histogram-quantile panels use correct
  `sum by (le[, worker]) (rate(..._bucket[..]))` form; `_bucket` suffixes
  correct.
- Config examples (lines 21-38) use kebab-case keys matching 022/021
  (`stale-after-secs`, `gauge-refresh-secs`, `push-interval-secs`,
  `ccache-interval-secs`); `bind: "0.0.0.0:9090"` and the `null` ⇒ main-listener
  note align with 022's `Option<String>` bind semantics.
- Single scrape target carrying both server-owned and pushed-worker series
  (lines 75-78) is correct: 022 renders both through one `handle.render()`. No
  per-worker discovery needed — matches proposal 002.
- `scrape_interval: 15s` vs `stale_after = 45s` (≈3×) gives the gauge idle-
  timeout margin (line 78). NOTE — see the 022 review B1: this margin only holds
  while scraping is live, because idle pruning in `metrics-exporter-prometheus`
  is render-driven (each scrape calls `render()` which drives the prune). If
  scraping pauses, gauges are not pruned. 023's cadence is correct; the
  dependency is a 022-side concern, flagged there. Worth one cross-reference
  sentence here but not a 023 defect.
- "No worker-side services" (line 45) upholds proposal 002's defining
  constraint; node_exporter is server-host-only (lines 52, 171-176).

## Cross-document consistency

- **`worker`-label mismatch (v1 M1) — fully closed** at both ends (022 added the
  label; 023's correlation panel no longer groups `cbsd_builds_active` by
  worker). Every `by (worker)` grouping in 023 hits a 022 family that carries
  `worker`.
- **`outcome` vs `result` — resolved** across all three docs; `result` is the
  single vocabulary for success/failure/revoked.
- **Names / units / suffixes — consistent.** All dashboard references use 022
  family names verbatim; `_bucket` on quantiles, `_seconds`/`_bytes`/`_total`
  suffixes, unit-less ratios all line up.

## Confidence score

| Item                                                   | Points | Description                                            |
| ------------------------------------------------------ | ------ | ------------------------------------------------------ |
| Starting score                                         | 100    |                                                        |
| D8: m1 success-ratio divides by zero at low traffic    | -5     | Flagship SLO tile shows `NaN` with no builds in window |
| D11: m2 hard-coded `4` pool ceiling drifts from config | -5     | Magic literal not tracked to source-of-truth           |
| **Total**                                              | **90** |                                                        |

**Interpretation: 90 / 100 — ready to merge; minor or no issues.** **Delta vs
v1: +10 (80 → 90).** Both v1 majors (the two broken `by (worker)` panels, −10)
are resolved; only the two pre-existing minors remain, neither blocking. 023 was
already the strongest of the three and is now clean apart from cosmetic polish.

### Most important remaining fix

Add a divide-by-zero guard (`clamp_min` / `or vector(0)`) to the success-ratio
SLO tile (line 104) so it doesn't render `NaN` during idle windows. Everything
else is optional. (No blocking issue — ready once this cosmetic guard is added.)
