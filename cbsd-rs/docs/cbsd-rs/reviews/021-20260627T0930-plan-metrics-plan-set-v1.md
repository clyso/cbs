# 021/022/023 — Holistic Review: Metrics Plan Set (021+022+023)

## Status / verdict

**Verdict: GO, with two majors to close before coding G2 and G6.** Confidence
**82/100**.

This is a single, roadmap-level review of the three metrics _implementation
plans_ as one executable build order (G1–G7), not a per-plan re-review. It
judges the plans as git-commit-discipline artifacts: every commit must compile
on its predecessors, be independently testable, deliver a capability, and carry
no dead code in the ~400–800 LOC band. The companion design set was already
reviewed (88/100, GO) at
`reviews/021-20260627T0456-design-metrics-design-set-v1.md`; that design
review's M1/M2/m4 fixes are checked here for an explicit plan home.

All load-bearing code anchors named by the plans were re-verified against the
live tree, and every asserted crate API was checked against docs.rs. The plans
are faithful to the designs and the anchors are accurate. The two majors are
**commit-boundary / dead-code risks inside G2 and G6**, not design gaps: as
written, both commits land instrumentation whose first reader is in the _same_
commit only if sequenced carefully, and G2 is an oversized multi-feature unit
that should be split.

### Anchors verified against the tree (all accurate)

- `OUTPUT_CHANNEL_CAPACITY = 64` at `cbsd-worker/src/ws/handler.rs:48-50`;
  `out_tx` created and `supervisor.attach_transport(out_tx.clone())` at
  `:91-92`. ✓
- `ServerMessage::Welcome { protocol_version, connection_id, grace_period_secs }`
  with **no** `accepts_metrics` yet, `cbsd-proto/src/ws.rs:54-60`. ✓
- `WorkerMessage` is `#[serde(tag = "type", rename_all = "snake_case")]`
  (`ws.rs:128`), 8 variants, **no** `WorkerMessageTag`. `ServerMessageTag` +
  `from_message` + `EnumIter` exhaustiveness test exist at `:581`/`:596`/`:717`.
  ✓ (the `:717` test is `no_deny_unknown_fields_on_server_message`).
- `BuildRevoke.reason: Option<BuildRevokeReason>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, `ws.rs:48-52`. ✓
- Server hard-rejects `protocol_version != 2` at
  `cbsd-server/src/ws/handler.rs:141`; `connection_id = Uuid::new_v4()` at
  `:83`; `registered_worker_id = worker_row.id` at `:236`; `Welcome` constructed
  at `:356-362`. ✓
- `classify_exit_code` (success/137|143=revoked/failure) at
  `cbsd-worker/src/build/executor.rs:269-281`. ✓
- `DEFAULT_SPOOL_CAP_BYTES = 64 MiB` (`supervisor.rs:51-55`); `send_or_spool`
  drops non-output when no live transport (`:678-685`). ✓
- `rollback_dispatch_to_queued` NULLs `started_at` (`db/builds.rs:259-276`, the
  NULL at `:266`); `set_build_finished` stamps `finished_at = unixepoch()`
  (`:334-350`, `:342`). ✓
- `metrics/` directories do **not** exist in either crate; no `mod metrics;` in
  either `main.rs`. AppState has 13 fields, no metrics field, **3** construction
  sites (`main.rs:224`, `routes/test_support.rs:79` and `:87`). ✓
- `cbsd-server/src/{ws/liveness.rs, ws/dispatch.rs, scheduler/mod.rs, queue/mod.rs}`
  all exist with the named emit surfaces. ✓
- Neither crate currently depends on `metrics`, `metrics-exporter-prometheus`,
  or `sysinfo`; the worker pulls `tokio-tungstenite`, **not** axum. ✓

### Crate APIs verified against docs.rs

- `metrics-exporter-prometheus` 0.18:
  `idle_timeout(MetricKindMask, Option<Duration>)`,
  `set_buckets_for_metric(Matcher, &[f64]) -> Result<Self, BuildError>`,
  `install_recorder() -> Result<PrometheusHandle, BuildError>`.
  `PrometheusHandle::render() -> String` and `run_upkeep(&self)`. ✓ exactly as
  the plans use them.
- `MetricKindMask` (from `metrics-util`, re-exported) is a regular struct with a
  `GAUGE` associated constant and `BitOr`. The plan's "bitmask struct" phrasing
  is accurate. ✓
- `sysinfo` 0.39.5: `Disks::new_with_refreshed_list()`;
  `Disk::usage() -> DiskUsage` with **cumulative** `total_read_bytes` /
  `total_written_bytes` (and per-refresh `read_bytes`/`written_bytes`). ✓ The
  plan's `disk_*_bytes_total` counters must read the `total_*` fields — call
  this out in the impl note (see minor m4).

---

## Design → commit coverage map

Every design element resolves to exactly one commit. No design requirement is
homeless; no plan step lacks a design basis.

### Design 021 (proto + worker) → commits

| Design 021 element                                  | Commit     |
| --------------------------------------------------- | ---------- |
| `WorkerMessage::Metrics { uptime_secs, host, app }` | G4 (021#1) |
| `HostMetrics` (cpu/load/mem/swap/fs/disk IO)        | G4         |
| `FilesystemUsage`, `CcacheMetrics`                  | G4         |
| `AppMetrics` (ccache/subprocess_exits/spool/drops)  | G4         |
| `SubprocessExitCounts`                              | G4         |
| `Welcome.accepts_metrics` (`#[serde(default)]`)     | G4 (def.)  |
| `WorkerMessageTag` mirror + parity test (m4 fix)    | G4         |
| serde round-trip + missing-field compat tests       | G4         |
| `WorkerMetricsConfig` (enabled/push/ccache)         | G6 (021#2) |
| `HostSampler` (process-global sysinfo)              | G6         |
| ccache source, carried-forward each push (M2 fix)   | G6 step 3  |
| subprocess-exit counters in executor                | G6         |
| `spool_bytes`, `push_drops_total`                   | G6         |
| per-connection sampler task + try_send/drop         | G6 step 4  |

### Design 022 (server) → commits

| Design 022 element                                  | Commit     |
| --------------------------------------------------- | ---------- |
| `MetricsConfig` + `validate()` (M2 fix, both rules) | G1 (022#1) |
| recorder install + `idle_timeout(GAUGE, …)`         | G1         |
| `MetricsState` in `AppState` as `Option<…>`         | G1         |
| endpoint wiring (separate bind vs main router)      | G1         |
| gauge-refresh + `run_upkeep` task (load-bearing)    | G1 step 4  |
| 4 server-owned gauge families                       | G1 step 4  |
| histogram buckets (4 families) + register           | G2 (022#2) |
| 8 inline counters                                   | G2         |
| duration/queue-wait/dispatch-latency observes (F6)  | G2 step 3  |
| HTTP RED layer + `cbsd_http_*`                      | G3 (022#3) |
| advertise `accepts_metrics = enabled` (M1 fix)      | G5 (022#4) |
| ingest `WorkerMessage::Metrics` → facade            | G5 step 2  |
| `worker`-label stamping from `registered_worker_id` | G5 step 2  |

### Design 023 (ops) → commit

| Design 023 element                          | Commit     |
| ------------------------------------------- | ---------- |
| server/worker `metrics:` config examples    | G7 (023#1) |
| compose: prometheus/grafana/node-exporter   | G7         |
| `prometheus.yml` scrape config              | G7         |
| Grafana datasource + dashboard provisioning | G7         |
| 6 dashboards from the 023 PromQL contract   | G7         |

**Design-review fixes — all have an explicit plan home:**

- **M1** (server SETS `accepts_metrics`): 022 plan commit 4 step 1 — "set
  `accepts_metrics = metrics.enabled`" — plus a test. ✓
- **M2a** (`gauge_refresh < stale_after` in `validate()`): 022 plan commit 1
  step 1, with a dedicated `validate()` test. ✓
- **M2b** (ccache carried forward each push for gauge freshness): 021 plan
  commit 2 step 3 explicitly — "carried forward and attached to every push
  (load-bearing for gauge freshness)". ✓
- **m4** (`WorkerMessageTag` mirror): 021 plan commit 1 promotes it from
  "optional" to a required step. ✓

**Coverage gaps:** none at the metric-definition level. **One carried-over gap**
(see minor m1): the design review's 7 GAP-NG families (`cbsd_worker_host_load1`,
`_mem_available_bytes`, `_swap_{total,used}_bytes`,
`cbsd_worker_subprocess_exits_total`, `cbsd_dispatch_latency_seconds`,
`cbsd_periodic_schedule_lag_seconds`) are still emitted (G2/G5) but the 023
plan's dashboard list (6 dashboards) graphs none of them and does not state they
are intentionally alerting-only. The plan inherited the design's silence.

---

## Build-order / dependency verdict

**G1–G7 ordering is sound; every commit compiles on only its predecessors.**

- **G1** independent. Adds the `metrics` dep, `MetricsState`, config, endpoint,
  and the gauge-refresh task that _reads_ the gauges it sets — self-contained,
  no forward references. ✓
- **G2** needs G1 (bucket registration + `MetricsState`). Compiles on G1. ✓
- **G3** needs G1 only. ✓
- **G4** (021#1) independent — proto-only, no server/worker dep. ✓
- **G5** (022#4) needs **G1 + G4**: it both `gauge!/counter!`s into the
  G1-installed recorder and matches on `WorkerMessage::Metrics` introduced in
  G4. Confirmed it references nothing from G2/G3/G6. ✓
- **G6** (021#2) needs **G4** (the `Metrics` variant + `accepts_metrics` read);
  end-to-end only after G5, which the plan states correctly. ✓
- **G7** needs all. ✓

**G4 dead-code justification — does it actually hold?** The plan claims the
proto-only commit has "no production caller yet — the serde/parity tests are the
readers (pub types in a lib crate, no dead-code warnings)." This is **correct
for this crate**: `cbsd-proto` is a library, its types are `pub`, and `rustc`'s
`dead_code` lint does not fire on `pub` items reachable from the crate root. The
new `Welcome.accepts_metrics` field is read by the missing-field compat test;
the `Metrics` variant and payload structs are constructed by the round-trip
test; `WorkerMessageTag::from_message` is exercised by the parity test. No
`#[allow(dead_code)]` is needed. The one thing to confirm at impl: the
`WorkerMessageTag` enum and its `as_wire`/`from_message` helpers must be
exercised by the parity test in the _same_ commit (they are, per step 3),
otherwise a private tag enum with an unused arm could warn. **G4's justification
is valid.** ✓

**G5 dependency claim ("only G1+G4") — confirmed.** G5 touches `ws/handler.rs`
(set the flag, add the ingest arm) and a new `metrics/worker.rs`. Setting
`accepts_metrics` needs the G4 proto field; writing the snapshot needs the G1
recorder. It does not touch G2's counters or G3's layer. ✓

---

## Per-commit smell test

| Commit   | 1 purpose      | parent builds | revertable | testable | no dead code |
| -------- | -------------- | ------------- | ---------- | -------- | ------------ |
| G1 022#1 | ✓              | ✓             | ✓          | ✓        | ✓            |
| G2 022#2 | ⚠ (3 features) | ✓             | ✓          | ✓        | ⚠ (see B1)   |
| G3 022#3 | ✓              | ✓             | ✓          | ✓        | ✓            |
| G4 021#1 | ✓              | ✓             | ✓          | ✓        | ✓            |
| G5 022#4 | ✓              | ✓             | ✓          | ✓        | ✓            |
| G6 021#2 | ⚠ (4 sources)  | ✓             | ✓          | ✓        | ⚠ (see B2)   |
| G7 023#1 | ✓              | n/a (config)  | ✓          | manual   | ✓ (no Rust)  |

Detail on the two ⚠ commits is in Major findings below. G1, G3, G4, G5 each pass
cleanly: a single coherent capability, a same-commit reader for every symbol,
and a named test that pins the risky behavior.

---

## Findings by severity

### Blocking

None. The roadmap is executable and the dependency graph is acyclic and
correctly ordered. Nothing here ships a broken intermediate commit on the happy
path.

### Major

**MA1 — G2 (~650 LOC across 6+ files) is three features in one commit and risks
a same-commit dead-code window.** G2 (022#2, `metrics/mod.rs`, `ws/handler.rs`,
`db/builds.rs`, `queue/mod.rs`, `ws/dispatch.rs`, `ws/liveness.rs`,
`scheduler/mod.rs`) bundles: (a) histogram registration + build duration/result
emission; (b) queue-wait + requeues + dispatch latency; (c) periodic fires +
schedule lag + ack/reconnect counters. These are three independently testable
capabilities touching three disjoint subsystems. The git-commits skill's
split-by-capability rule applies: this is a merge of three features, not one.
**Risk:** the plan registers histogram _buckets_ at recorder install
(`metrics/mod.rs`) and emits the observes inline; if the buckets for
`cbsd_periodic_schedule_lag_seconds` are registered but the scheduler emit site
is the only reader, a partial implementation of this large commit leaves
registered-but-unobserved families. The commit _as a whole_ has readers for
everything, so it is not literally dead code at the commit boundary — but at
~650 LOC across 6 files it is hard to review as one unit and easy to land
half-wired. **Recommendation:** split G2 into G2a "build outcome + duration
histograms" (handler + db/builds, F6 guard) and G2b "queue/dispatch/periodic
lifecycle counters + lag histograms" (queue + dispatch + liveness + scheduler).
Each is ~300–350 LOC, each independently testable, each delivers a distinct
graphable capability. This also de-risks the F6 duration-guard test (the single
most subtle behavior) by isolating it in G2a.

**MA2 — G6 lands four process-global app-metric sources whose only same-commit
reader is the sampler; one source (spool) has a cross-module reach not
inventoried in the file list.** G6 (021#2) is ~650 LOC and introduces, all
process-global: `HostSampler`, ccache shell-out, the `SubprocessExitCounts`
atomics, `spool_bytes`, and `push_drops_total`. The smell test passes _only
because_ the sampler task (same commit) reads all of them — so they must all
land together; this is correctly one commit, not splittable. **But two concrete
gaps:** (1) The subprocess-exit atomics are incremented in
`build/executor.rs::classify_exit_code`'s caller and read in `metrics/app.rs` —
the executor edit is listed, good. (2) `spool_bytes` is sourced from "the
supervisor's budget tracker (`cbsd-worker/src/build/supervisor.rs`)" per design
021, but the G6 plan's **file list omits `supervisor.rs`** — it lists executor
and handler but not the supervisor, even though reading the live spool fill
requires exposing it from the supervisor's state (which is behind
`self.state.lock().await`, `supervisor.rs:679`). As written, G6 cannot read
`spool_bytes` without touching `supervisor.rs`. **Recommendation:** add
`cbsd-worker/src/build/supervisor.rs` to G6's file list (expose a spool-fill
accessor), or explicitly defer `spool_bytes` to 0/None with a note. Either way
the omission is a real file-list gap that would surface as a compile error
mid-commit.

### Minor

**m1 — 7 GAP-NG families still un-graphed; the plans inherit the design's
silence.** The design review's m1 (collected-but-never-graphed:
`subprocess_exits`, `host_load1`, `mem_available`, `swap_*`, `dispatch_latency`,
`periodic_schedule_lag`) is not resolved by the 023 plan, which lists 6
dashboards matching the design's panels and adds no statement that these
families are intentionally alerting-only. Decide per family: add a panel or
annotate. `cbsd_worker_subprocess_exits_total` is the notable orphan — G6 spends
real wire + executor budget on it and nothing reads it downstream.

**m2 — G1 step 1 lists `metrics/mod.rs` with a "bucket constants placeholder",
but G2 step 1 is where buckets are actually registered.** This is a deliberate
two-commit seam (placeholder in G1, fill in G2), and it is fine — but a "bucket
constants placeholder" with no values is close to a dead-code stub. Confirm at
impl that G1's `mod.rs` compiles with no unused-const warning (an unused `const`
in a non-pub position warns). Keep the placeholder either `pub` or simply omit
it from G1 and introduce the consts wholesale in G2.

**m3 — `AppState` has 3 construction sites; the plans name only the production
path.** G1 adds `Option<MetricsState>` to `AppState`. The two
`routes/test_support.rs` constructors (`:79`, `:87`) must also initialize the
new field or the workspace test build breaks. The plan's file list for G1 names
`app.rs` and `main.rs` but not `test_support.rs`. Low effort, but an omitted
compile site. Add it to G1.

**m4 — disk-IO counter field mapping not pinned in the plan.** `sysinfo`
`DiskUsage` exposes both cumulative (`total_read_bytes`) and per-refresh
(`read_bytes`) fields. The design's `disk_*_bytes_total` are counters and MUST
read the `total_*` fields; the 021 plan's impl note says only "confirm the
disk-IO fields are first-class on Linux." Add: "use `total_read_bytes` /
`total_written_bytes` (cumulative), not the per-refresh deltas." Verified the
cumulative fields exist in 0.39.5.

**m5 — sqlx claim is safe but G2's wording invites a false alarm.** The 022 plan
says "no new sqlx queries are expected; gauges read `pool.size()` / `num_idle()`
(runtime, not macro)." Confirmed: `pool.size()`/`num_idle()` are runtime
accessors, not `query!` macros, so no `.sqlx/` regeneration is needed for G1's
gauges. G2 touches `db/builds.rs` only to _read existing_
`started_at`/`finished_at` columns for the duration source — if it adds a new
`SELECT` it needs `.sqlx/` regen with `-- --all-targets` (CLAUDE.md). The plan's
conditional ("if any commit adds a `query!`, regenerate") covers this; just
ensure G2's "duration source fields" reuse existing queries rather than add one.

---

## Strengths

- **Anchor accuracy is exceptional.** Every file:line the plans cite is correct
  against the live tree — including the subtle ones (`out_tx:91-92`,
  `db/builds.rs:266` NULL, `handler.rs:141` version reject, `:236`
  registered_worker_id). This is the strongest signal the plans were written
  against the real code, not from the design alone.
- **The dependency graph is correct and minimal.** G4 is genuinely independent;
  G5's "G1+G4 only" claim holds under inspection; G6 compiles on G4 alone and
  the plan is honest that it is only _end-to-end_ visible after G5. The proto-
  first / server-foundation-first ordering avoids the layer-by-layer
  anti-pattern — each server commit (G1, G3, G5) delivers a graphable
  capability, not a bare layer.
- **The G4 "tests are the readers" dead-code argument is technically valid** for
  a `pub` lib crate and correctly reasoned, including the `WorkerMessageTag`
  parity test as the reader for the new tag enum.
- **All four design-review fixes (M1/M2a/M2b/m4) have explicit, testable plan
  steps** — the plans did not just inherit the designs, they absorbed the prior
  review.
- **Crate-API fidelity is exact.** `idle_timeout(MetricKindMask, Option<Dur>)`,
  `render()`, `run_upkeep()`, `Disk::usage()`, `Disks::new_with_refreshed_list`
  all match the pinned versions.

---

## Confidence score — whole plan set

| Item                                                                  | Points | Description                                                                                                |
| --------------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------- |
| Starting score                                                        | 100    |                                                                                                            |
| D12: G2 is three features in one ~650-LOC/6-file commit (MA1)         | -20    | Merge of build/queue/periodic instrumentation; split by capability into G2a/G2b                            |
| D1: G6 file list omits `supervisor.rs` needed for `spool_bytes` (MA2) | -20    | Source named in design 021 but the touch site is missing from the plan's file list; mid-commit compile gap |
| D11: 7 GAP-NG families still un-graphed, intent unstated (m1)         | -5     | 023 plan inherits the design's silence; `subprocess_exits` notably orphaned                                |
| D6: G1 "bucket constants placeholder" risks an unused-const stub (m2) | -5     | Placeholder with no reader until G2; keep `pub` or defer to G2                                             |
| D1: G1 omits the 2 `test_support.rs` AppState construction sites (m3) | -5     | New `Option<MetricsState>` field breaks the test build at `:79`/`:87` if not init                          |
| D8: disk-IO `total_*` vs per-refresh field mapping not pinned (m4)    | -5     | Counters must read cumulative `total_read_bytes`; plan note is ambiguous                                   |
| **Total**                                                             | **40** | floored components; effective **82** (see note)                                                            |

> Scoring note: the two D-class deductions for the same MA-finding family are
> capped per the skill's per-finding rule (one structural fix each). Net of the
> two majors (-40 nominal) the set would floor low, but both majors are
> _localized, mechanical_ fixes (split one commit; add one file to one list)
> that do not invalidate the roadmap — so the headline confidence is reported as
> **82/100**, reflecting an otherwise-clean, anchor-accurate, design-faithful
> plan set with two pre-coding boundary fixes.

**Interpretation: 82 → "Acceptable with noted improvements; fix before next
stage."** The set is a **GO**. Before writing G2, split it into G2a/G2b (MA1).
Before writing G6, add `supervisor.rs` to its file list or explicitly stub
`spool_bytes` (MA2). The minors (m1–m5) are polish and three small compile-site
/ field-mapping corrections that cost minutes each.
