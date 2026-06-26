# 021/022/023 — Holistic Review: Metrics Design Set (021+022+023)

## Status / verdict

**Verdict: GO, with minor pre-implementation cleanups.** Confidence **88/100**.

This is a single, system-level review of the three metrics design documents **as
one end-to-end pipeline**, not a per-document re-review (021, 022, 023 were each
reviewed twice already; their per-doc facts are trusted here unless a new
system-level doubt arose):

- 021 — wire protocol & worker collector (`cbsd-proto` + `cbsd-worker`)
- 022 — server exporter & aggregation (`cbsd-server`)
- 023 — deployment & dashboards (compose, Prometheus, Grafana)

The set is coherent, implementable, and the seams (handshake, identity,
staleness, config) line up. The findings below are **system-level**: gaps that
only appear when you trace a metric or a failure across all three docs. None is
blocking. The dominant theme is a handful of **sourced-and-exposed-but-never-
graphed** metrics, one **config-invariant that is asserted but not enforced
end-to-end**, and a thin spot in the `accepts_metrics` handshake where the
**server side that sets the flag is never explicitly specified** in any of the
three docs.

All load-bearing cross-doc code anchors were re-verified against the live tree
(see Evidence notes inline): `OUTPUT_CHANNEL_CAPACITY = 64`
(`cbsd-worker/src/ws/handler.rs:50`); `out_tx` per-connection, cloned to
supervisor (`:91-92`); server hard-rejects `protocol_version != 2`
(`cbsd-server/src/ws/handler.rs:141`); `registered_worker_id = worker_row.id`
(DB identity, stable across reconnects, `:236`) vs per-connection
`connection_id = Uuid::new_v4()` (`:83`);
`Welcome { protocol_version, connection_id, grace_period_secs }` with **no**
`accepts_metrics` field yet (`cbsd-proto/src/ws.rs:55-60`); `BuildRevoke.reason`
is `Option` with `#[serde(default, skip_serializing_if)]` (`:48-52`);
`ServerMessageTag` exhaustiveness witness exists, `WorkerMessage` has none;
`send_or_spool` drops non-output when disconnected (`supervisor.rs:678-685`), 64
MiB spool cap (`:55`); `classify_exit_code` → success/137|143=revoked/failure
(`executor.rs:275-281`); `rollback_dispatch_to_queued` NULLs `started_at`
(`db/builds.rs:266`), `set_build_finished` stamps `finished_at` (`:342`);
`workers: HashMap<ConnectionId, WorkerState>` (`queue/mod.rs`),
`handle_worker_dead` (`ws/handler.rs`).

---

## End-to-end metric lineage

Legend for **Status**: OK = sourced (021 field or 022 server event), exposed
(022 catalog), and consumed (≥1 023 panel). **GAP-NG** = sourced + exposed but
**never graphed**. **GAP-NS** = graphed/exposed but **not sourced**. **DERIVED**
= no single source field; computed in PromQL.

### Pushed worker metrics (021 field → 022 family → 023 panel)

| 021 source field                | 022 exposed family                           | 023 panel                                 | Status |
| ------------------------------- | -------------------------------------------- | ----------------------------------------- | ------ |
| `uptime_secs`                   | `cbsd_worker_uptime_seconds` (gauge)         | Per-Worker "Worker uptime"; templ. var    | OK     |
| `host.cpu_busy_ratio`           | `cbsd_worker_host_cpu_busy_ratio`            | Host&ccache "CPU busy"; Correlation       | OK     |
| `host.load1`                    | `cbsd_worker_host_load1` (gauge)             | **none**                                  | GAP-NG |
| `host.mem_total_bytes`          | `cbsd_worker_host_mem_total_bytes`           | Host "Memory used/total" (denominator)    | OK     |
| `host.mem_used_bytes`           | `cbsd_worker_host_mem_used_bytes`            | Host "Memory"; Correlation "Mem vs spool" | OK     |
| `host.mem_available_bytes`      | `cbsd_worker_host_mem_available_bytes`       | **none**                                  | GAP-NG |
| `host.swap_total_bytes`         | `cbsd_worker_host_swap_total_bytes`          | **none**                                  | GAP-NG |
| `host.swap_used_bytes`          | `cbsd_worker_host_swap_used_bytes`           | **none**                                  | GAP-NG |
| `host.filesystems[].used/total` | `cbsd_worker_host_fs_{used,total}_bytes`     | Host "Filesystem fill"                    | OK     |
| `host.disk_read_bytes_total`    | `cbsd_worker_host_disk_read_bytes_total`     | Host "Disk IO"                            | OK     |
| `host.disk_written_bytes_total` | `cbsd_worker_host_disk_written_bytes_total`  | Host "Disk IO"                            | OK     |
| `app.ccache.size_bytes`         | `cbsd_worker_ccache_size_bytes`              | Host "ccache size vs max"                 | OK     |
| `app.ccache.max_bytes`          | `cbsd_worker_ccache_max_bytes`               | Host "ccache size vs max"                 | OK     |
| `app.ccache.hit_ratio`          | `cbsd_worker_ccache_hit_ratio`               | Host "ccache hit ratio"                   | OK     |
| `app.subprocess_exits.{s,f,r}`  | `cbsd_worker_subprocess_exits_total{result}` | **none**                                  | GAP-NG |
| `app.spool_bytes`               | `cbsd_worker_spool_bytes`                    | Correlation "Memory vs spool"             | OK     |
| `app.push_drops_total`          | `cbsd_worker_metrics_push_drops_total`       | Host "Push drops"                         | OK     |

### Server-owned metrics (022 source → 022 family → 023 panel)

| 022 source                           | 022 family                             | 023 panel                              | Status |
| ------------------------------------ | -------------------------------------- | -------------------------------------- | ------ |
| queue lanes                          | `cbsd_builds_queued{priority,arch}`    | Queue "Queue depth by priority"        | OK     |
| `queue.active`                       | `cbsd_builds_active{arch}`             | Queue "Active builds"                  | OK     |
| `WorkerState`                        | `cbsd_workers_connected{state,arch}`   | Fleet "Workers connected by state"     | OK     |
| `pool.size/num_idle`                 | `cbsd_db_pool_connections{state}`      | Fleet "DB pool saturation"             | OK     |
| `build_finished`                     | `cbsd_build_results_total{...,worker}` | Queue/Per-Worker/Correlation           | OK     |
| `rollback_dispatch_to_queued`        | `cbsd_build_requeues_total{reason}`    | Queue "Re-dispatch rate"               | OK     |
| build-timeout path                   | `cbsd_build_timeouts_total{arch}`      | Duration "Timeouts & SIGKILLs"         | OK     |
| revoke/escalation                    | `cbsd_sigkill_escalations_total`       | Duration "Timeouts & SIGKILLs"         | OK     |
| dispatch ack timer                   | `cbsd_dispatch_ack_timeouts_total`     | Fleet "Ack timeouts"                   | OK     |
| revoke ack timer                     | `cbsd_revoke_ack_timeouts_total`       | Fleet "Ack timeouts"                   | OK     |
| connection migration                 | `cbsd_worker_reconnects_total{worker}` | Per-Worker "Reconnects by worker"      | OK     |
| scheduler                            | `cbsd_periodic_fires_total{result}`    | Fleet "Periodic outcomes"              | OK     |
| HTTP RED layer                       | `cbsd_http_requests_total{...}`        | Fleet "HTTP RED"                       | OK     |
| `cbsd_build_duration_seconds`        | histogram `{result,arch,worker}`       | Duration ×3; Per-Worker "Duration p95" | OK     |
| `cbsd_build_queue_wait_seconds`      | histogram `{priority,arch}`            | Queue "Queue wait p50/p95"             | OK     |
| `cbsd_dispatch_latency_seconds`      | histogram `{arch}`                     | **none**                               | GAP-NG |
| `cbsd_periodic_schedule_lag_seconds` | histogram                              | **none**                               | GAP-NG |
| `cbsd_http_request_duration_seconds` | histogram `{route,method}`             | Fleet "HTTP RED" (p95)                 | OK     |

### Lineage gaps (no orphans either direction)

- **No GAP-NS** — every metric a 023 panel references is sourced and exposed. No
  graphed-but-undefined series. Good.
- **GAP-NG (defined but never graphed), 7 families:** `cbsd_worker_host_load1`,
  `cbsd_worker_host_mem_available_bytes`,
  `cbsd_worker_host_swap_{total,used}_bytes`,
  `cbsd_worker_subprocess_exits_total`, `cbsd_dispatch_latency_seconds`,
  `cbsd_periodic_schedule_lag_seconds`. These are **collected, pushed/emitted,
  and rendered on `/metrics` but no dashboard consumes them.** That is not a
  correctness bug — they are useful ad-hoc/alerting inputs — but it is a
  **system-level inconsistency the set should resolve explicitly** (see M1):
  either add the panels or state in 023 that these are intentionally
  collected-for-alerting-only. `subprocess_exits` is the most surprising
  omission given 021 spends real wire budget on it and 022 calls out its label
  vocabulary alignment with `cbsd_build_results_total`.

---

## Findings by system impact

### Blocking

None. The pipeline is coherent end-to-end; nothing here would produce a wrong or
missing series at runtime in the common path.

### Major

**M1 — `accepts_metrics` handshake: the server side that SETS the flag is
specified in no document.** 021 defines the field on `Welcome` and the worker's
**read** side ("spawn only if `Welcome.accepts_metrics`", 021 §"Per-connection
sampler"). 022 owns the server but **never says the server sets
`accepts_metrics = true` when `metrics.enabled`** — grep 022 for the field: it
does not appear. The server's `Welcome` is built in
`cbsd-server/src/ws/handler.rs` (the same site that mints `connection_id` at
`:83` and rejects `protocol_version != 2` at `:141`), squarely 022's territory,
yet 022's "Work order" (steps 1–4) and registry sections omit it. So the
**producer of the capability bit falls in the seam**: 021 assumes 022 sets it,
022 never mentions it. Net effect if taken literally: field defaults to `false`,
**no worker ever pushes, and the entire 021/023 worker-metrics half is dark** —
yet every per-doc review passed because each doc is internally consistent. This
is the single most important system-level gap. **Fix:** 022 must add, to its
work order and ideally its registry/wiring section, "when `metrics.enabled`, the
`Welcome` builder sets `accepts_metrics = true`" — and a test that a
metrics-enabled server emits `accepts_metrics: true`. (Also: 021 places the
field **definition** on `Welcome` in `cbsd-proto`; that is correct and
unambiguous — only the server-side _value-setting_ is unowned.)

**M2 — `gauge_refresh < stale_after` invariant is asserted but not validated
end-to-end; the inverse `ccache_interval` freshness invariant is not stated at
all.** Two distinct cross-doc timing invariants:

- 022 `validate()` rejects `bind == listen_addr` but the doc never says
  `validate()` rejects `gauge_refresh_secs >= stale_after_secs`. 022 §Risks
  calls the relation "load-bearing" ("server-owned gauges MUST be re-set on the
  `gauge_refresh` cadence (< `stale_after`), or they would vanish between
  scrapes") — but leaves it to the operator. With defaults (5 < 45) it holds; a
  careless override silently prunes **server-owned** gauges mid-scrape. This is
  a config-coherence invariant that spans 022 (both knobs) and should be
  enforced in `validate()`, not just narrated.
- **The ccache-vs-push freshness invariant is entirely implicit.** 021 carries
  the last `CcacheMetrics` forward between `ccache_interval_secs` (60) refreshes
  and pushes it every `push_interval_secs` (15). So the ccache **gauge** is
  `set()` every push (15 s) with a _stale-but-carried_ value — good, it stays
  fresh w.r.t. `stale_after` (45 s) **regardless** of `ccache_interval`. The
  task's framing ("ccache_interval > push_interval so the carried-forward value
  keeps the gauge fresh") is the right intuition, and the design **does** keep
  it fresh because the carry-forward means every push re-`set()`s the gauge. But
  **no doc states this invariant**, and it would break if an implementer
  "optimized" by only including `ccache` in the snapshot on refresh ticks (via
  the `skip_serializing_if = "Option::is_none"` already on the field, 021:98) —
  then the ccache gauge would update only every 60 s, still < 45 s? No: 60 > 45,
  so it **would** idle-prune between refreshes. The carry-forward is therefore
  **load-bearing for staleness**, not just for cost, and 021/022 should say so
  explicitly so the optimization is not made. **Fix:** state in 021 that ccache
  is included in _every_ push (carried forward) precisely so the gauge stays
  inside `stale_after`, and add the `gauge_refresh < stale_after` check to 022
  `validate()`.

### Minor

**m1 — 7 GAP-NG metrics (see lineage).** Decide per family: add a 023 panel or
annotate as alerting-only. Most worth a panel:
`cbsd_worker_subprocess_exits_total` (build-failure signal per worker) and
`cbsd_worker_host_load1` (cheap, classic).

**m2 — `scrape_interval ≈ push_interval` coupling is convention-only and
duplicated across two files.** 023 hard-codes `scrape_interval: 15s` in
`prometheus.yml` and the worker default `push-interval-secs: 15` in
`worker.example.yaml`; the relation ("matches the worker push-interval") lives
only in a comment. If an operator raises `push-interval` to 30 s without
touching Prometheus, scrapes double-sample (harmless) ; if they lower
`stale_after` below `2× scrape_interval` the gauge can flap. The
`stale_after ≈ 3× push_interval` margin (45 vs 15) is correct **only** while all
three track. This is inherent to three independently-configured processes; the
mitigation is documentation — 023 should add a one-line "tuning rule:
`gauge_refresh < stale_after`, `scrape_interval ≈ push_interval`,
`stale_after ≈ 3× push_interval`" box so the three knobs are tuned as a unit.

**m3 — `cbsd_db_pool_connections{state="acquired"}` is derived, not a direct
accessor.** 022 sources it from `pool.size()` / `pool.num_idle()`; `acquired`
must be computed as `size − num_idle` (sqlx exposes no `acquired()`). 023's
Fleet "DB pool saturation" panel compares it against literal `4`. Both are fine
but (a) the derivation should be named in 022 so it is not mistaken for an API
call, and (b) the `4` in 023 duplicates CLAUDE.md invariant #2's
`max_connections = 4` — if the pool size ever changes, the panel silently lies.
Low impact; note it.

**m4 — `WorkerMessage` has no tag-exhaustiveness witness (carried from 021, but
a _system_ risk).** 021 correctly flags that, unlike `ServerMessage`,
`WorkerMessage` has no `*Tag` enum / parity test, so adding `Metrics` gets no
free exhaustiveness enforcement. At the **set** level this matters because 022's
ingestion hook and 023's whole worker-metrics surface depend on that one variant
round-tripping. The single round-trip test 021 mandates is adequate, but a
`WorkerMessageTag` mirror (021 calls it "optional") would protect the seam
cheaply. Recommend promoting it from optional to "do it" given how much of
022/023 hangs off this one variant.

---

## Seam analysis

### Seam 1 — the `accepts_metrics` handshake (021 ↔ 022)

Full trace: server builds `Welcome` (`cbsd-server/src/ws/handler.rs`, near
`:83`/`:141`) → worker reads it on the strict path in `run_connection` → worker
spawns sampler iff `metrics.enabled && Welcome.accepts_metrics`. **Field
definition:** owned by 021 in `cbsd-proto` (`#[serde(default)] bool`), correct
and forward/back-compatible (absent ⇒ `false` ⇒ silent; verified `Welcome` has
no such field today at `ws.rs:55-60`, so this is a clean additive change).
**Field _setting_ (server → true):** unowned — see **M1**. The _read_, the
_default_, and the _compat test_ are all specified; only the server's write is
missing. This is the textbook "each doc assumes the other handles it" seam the
holistic pass exists to catch.

### Seam 2 — cross-document config coherence

| Knob                   | Doc(s)    | Default | Invariant                                  |
| ---------------------- | --------- | ------- | ------------------------------------------ |
| `push_interval_secs`   | 021 / 023 | 15      | ≈ `scrape_interval`; drives carry-forward  |
| `ccache_interval_secs` | 021 / 023 | 60      | carry-forward ⇒ gauge still set every push |
| `gauge_refresh_secs`   | 022       | 5       | **must be < `stale_after`**                |
| `stale_after_secs`     | 022 / 023 | 45      | ≈ 3× `push_interval`; > `gauge_refresh`    |
| `scrape_interval`      | 023       | 15s     | ≈ `push_interval`                          |

With defaults all invariants hold: `5 < 45`, `45 ≈ 3×15`, `15 ≈ 15`, and the
carry-forward keeps the ccache gauge `set()` every 15 s (well inside 45 s). The
**weaknesses** are: (a) `gauge_refresh < stale_after` is unenforced (**M2**);
(b) the carry-forward-is-load-bearing-for-freshness fact is unstated (**M2**);
(c) the four-way `push/scrape/stale/gauge_refresh` relationship is spread across
three files with no single "tune these together" statement (**m2/m3**). No
combination in the documented defaults breaks; the risk is entirely in
uncoordinated overrides.

### Seam 3 — end-to-end failure modes

| Scenario                              | Behavior across 021→022→023                                                                                                                     | Coherent? |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | --------- |
| Worker goes offline                   | sampler stops (021 lifecycle=connection); gauges idle-prune after `stale_after` via render/upkeep (022 B1 fix); panels drop the series.         | Yes       |
| Worker reconnects (new conn id)       | same `registered_worker_id` ⇒ same `worker` label ⇒ one continuous series (022); `cbsd_worker_reconnects_total{worker}` ticks (Per-Worker).     | Yes       |
| Server restarts                       | recorder re-installed; one-interval empty window until next push; counters via `.absolute()`; gauge-refresh first tick resyncs server gauges.   | Yes       |
| Prometheus scrape pauses then resumes | **022 B1 upkeep task** prunes idle gauges independent of scrape liveness; without it stale gauges would linger — the doc fixes this explicitly. | Yes       |
| Build revoked before start            | `rollback_dispatch_to_queued` NULLs `started_at` (`db/builds.rs:266`); F6 guard skips duration sample; still counted in `_results_total`.       | Yes       |
| Worker restarts (counter reset)       | cumulative counters reset to 0; `.absolute()` + `rate()` absorb; `uptime_secs` regression disambiguates restart vs gap (021/022).               | Yes       |

The render/upkeep-driven pruning (022's core B1 fix) **is** consistent with
023's 15 s scrape model: each scrape `render()`s (prunes), and the
`gauge_refresh`-cadence `run_upkeep()` covers the scrape-paused case. The two
docs agree. This seam is the strongest part of the set.

### Seam 4 — label / identity consistency

Single join key end-to-end: **`worker` label = `registered_worker_id`**, stamped
**server-side** in 022's ingestion hook (worker never sends it — F8), sourced
from `worker_row.id` (`cbsd-server/src/ws/handler.rs:236`, stable across
reconnects). Same label on server-owned per-worker metrics
(`cbsd_build_results_total{worker}`, `cbsd_build_duration_seconds{worker}`,
`cbsd_worker_reconnects_total{worker}`) and on pushed `cbsd_worker_*{worker}`.
023's `by (worker)` groupings, the `$worker` template var
(`label_values(cbsd_worker_uptime_seconds, worker)`), and the Correlation
dashboard's per-`worker` join all key off this one value. **No split identity.**
`connection_id` (per-connection UUID) is correctly used only for liveness
(`workers: HashMap<ConnectionId, …>`), never as a series key — and 022 leans on
exactly this to dissolve the reconnect race. Clean.

### Seam 5 — completeness vs proposal 002

Proposal 002's eight work items map onto the set: items 1–3 (server exporter,
server-owned instrumentation, HTTP RED + DB pool) → 022 work-order 1–3; item 4
(protocol) → 021 wire section; item 5 (worker collector) → 021 collector; item 6
(server aggregation) → 022 work-order 4; item 7 (stack/compose) → 023; item 8
(this design set itself). Every proposal promise lands somewhere. The one
responsibility that falls **between** docs is the server-side `accepts_metrics`
write (**M1**) — proposal 002 §"Compatibility" describes the handshake but
neither 021 nor 022 claims the server-write half.

---

## System-level strengths

- **Identity model is airtight (Seam 4).** One server-stamped `worker` label
  threads server-owned and pushed metrics into a clean per-worker join, and the
  `connection_id`/`registered_worker_id` split is used exactly right — series
  continuity on the stable id, liveness on the per-connection id. This is the
  hardest thing to get right in a push-aggregation design and it is correct.
- **Staleness is solved structurally, not with a custom cache (Seam 3).**
  Replacing the earlier `WorkerMetricsCache` with `idle_timeout(GAUGE, …)` +
  render/upkeep dissolves both the reconnect race (F3) and the freshness problem
  with library mechanics, and 022's B1 fix (explicit `run_upkeep` task) closes
  the one subtlety that mechanism introduces. The GAUGE-only scoping (counters
  left as benign flat series) is the right call and is consistent across
  022/023.
- **No graphed-but-undefined metrics.** Every PromQL in 023 resolves to a family
  defined in 022 with matching labels — the consumer side is fully backed.
- **Failure-mode coverage is genuinely end-to-end** and the docs agree with each
  other on every scenario walked above.

---

## Confidence score — whole design set

| Item                                                                       | Points | Description                                                                                 |
| -------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------- |
| Starting score                                                             | 100    |                                                                                             |
| D1: server-side `accepts_metrics=true` write specified in no doc (M1)      | -20    | Producer of the capability bit falls in the 021/022 seam; literal read ⇒ no worker pushes   |
| D8: `gauge_refresh < stale_after` invariant asserted but unenforced (M2)   | -5     | 022 calls it load-bearing but `validate()` only checks the bind clash                       |
| D8: ccache carry-forward-as-freshness invariant unstated (M2)              | -5     | Load-bearing for staleness, framed only as a cost optimization; a plausible "opt" breaks it |
| D11: 7 sourced+exposed-but-never-graphed metrics, intent unstated (m1)     | -5     | Set should add panels or annotate alerting-only; `subprocess_exits` notably orphaned        |
| D10: push/scrape/stale/gauge_refresh tuning rule not stated as a unit (m2) | -5     | Coupling spread across 3 files as comments; uncoordinated override flaps gauges             |
| D3: `db_pool acquired` derivation + literal `4` duplication unflagged (m3) | -5     | `size − num_idle`; `4` duplicates CLAUDE.md invariant #2, silently drifts                   |
| D5: `WorkerMessage` tag-exhaustiveness witness left optional (m4)          | -5     | Whole 022/023 worker surface hangs off one variant with only a round-trip test              |
| **Total**                                                                  | **88** | Acceptable; fix M1 (and ideally M2) before implementation                                   |

**Interpretation: 88 → "Acceptable with noted improvements; fix before next
stage."** The set is a GO. The one thing that must be fixed before code is
**M1** — name the server-side `accepts_metrics = true` write in 022, with a test
— because taken literally the documented set ships a dark worker-metrics
pipeline. **M2** (two unenforced/unstated config invariants) should be closed in
the same pass since it is cheap and prevents a silent staleness regression. The
minors are polish and dashboard completeness.
