# 021 — Review: Metrics Wire Protocol & Worker Collector (v2)

## Verdict

**Go.** The round-1 major (M1: a false "existing parity test enforces the new
variant" claim) is fully resolved, and the `outcome`→`result` label drift is
fixed at the source. The wire-protocol design remains structurally correct on
every codebase anchor I re-verified (enum tagging, `Welcome` shape,
`BuildRevoke` serde precedent, additive `#[serde(default)] bool`,
protocol-version reject, per-connection `out_tx`, `classify_exit_code`). What
remains are two pre-existing minors carried over from v1 (the `sysinfo` version
hedge and the per-connection CPU-`System` ownership ambiguity), neither of which
blocks implementation. No new defects introduced by the revision.

Reviewed against source on branch `wip/cbsd-rs-metrics` and current docs.rs
stable (`sysinfo` 0.39.5, `metrics` 0.24.6).

## Disposition of round-1 findings

- **M1 — false "existing parity test enforces the `Metrics` variant" claim —
  RESOLVED.** The new "Tag exhaustiveness (net-new, no free enforcement)"
  paragraph (lines 132-138) now states the truth verbatim: `WorkerMessage` "has
  **no** tag enum and only per-variant round-trip tests. So no existing test
  will flag a missing `Metrics` arm," and a `WorkerMessageTag` mirror is
  "optional, net-new work — not free enforcement." This matches the code
  exactly: `cbsd-proto/src/ws.rs` defines `ServerMessageTag` (`ws.rs:582`),
  `from_message(&ServerMessage)` (`ws.rs:596`), `sentinel_for_tag`
  (`ws.rs:623`), and the parity test `no_deny_unknown_fields_on_server_message`
  (`ws.rs:660`) — all `ServerMessage`-only; `WorkerMessage` (`ws.rs:130`) has
  only per-variant round-trip tests (`worker_message_hello_round_trip`
  `ws.rs:275`, etc.) and no exhaustiveness test. The "Required serde tests"
  section (lines 178-185) is correspondingly honest: it asks for a `metrics`
  round-trip test and a tag-parity test while no longer implying either exists.
  The net-new test work is now visible rather than hidden. Clean fix.

- **`outcome` → `result` label drift — RESOLVED (at source).** Line 103 now
  republishes subprocess exits "with a `result` label (same values as the
  server's `cbsd_build_results_total{result}`, for a consistent label
  vocabulary)," and 022's catalog labels the family
  `cbsd_worker_subprocess_exits_total{worker, result}`. The success/failure/
  revoked classification now carries the identical label name across both the
  server-owned and worker-pushed families. `grep` finds zero remaining `outcome`
  label usages in the three designs (the only `outcome` token left is the prose
  panel title "Periodic outcomes" in 023, which correctly uses the `result`
  label underneath). Trap removed.

- **`Counter::absolute()` notation — RESOLVED.** Line 144 now reads
  "`.absolute(v)`" (instance-method form) rather than the misleading
  `Counter::absolute()`. Confirmed correct against docs.rs: `metrics` 0.24.6
  `Counter::absolute(&self, value: u64)` and the `counter!` macro returns a
  `Counter`, so `counter!(...).absolute(v)` is the right call.

## New findings

### MINOR (carried from v1, still open)

**m1 — `sysinfo` `/proc` fallback hedge still understates current capability.**
Lines 229-234 keep the "the earlier `/proc` hedge is a last resort only; pin the
version and confirm the fields at implementation" language. Verified against
docs.rs: at current stable `sysinfo` 0.39.5,
`System::load_average() -> LoadAvg { one, five, fifteen }` and
`Disk::usage() -> DiskUsage { total_read_bytes, total_written_bytes, .. }` both
exist on Linux, so neither `load1` nor `disk_*_bytes_total` needs a `/proc`
fallback at the pinned-latest version. The v2 text is softer than v1 ("last
resort only") and explicitly says to confirm at the pinned version, so this is
now a documentation nicety rather than a correctness risk — but the design could
simply name the concrete version (0.39.x) and drop the `/proc` hedge entirely.
Low impact.

**m2 — per-connection sampler vs warmed CPU `System` ownership still
ambiguous.** Lines 218-219 say `HostSampler` "wraps a single long-lived
`sysinfo::System`," and the first CPU sample after start is "discarded/zeroed"
(sysinfo needs two refreshes for a delta). But the sampler **task** is
per-connection (lines 260-265, correctly mirroring `out_tx` at
`ws/handler.rs:91-92`), spawned fresh on every reconnect. If the `System` is
re-instantiated with the task, the first push after each reconnect carries a
zeroed `cpu_busy_ratio`. The doc does not state whether the warmed `System`
outlives the per-connection task. Recommend an explicit sentence: the
`HostSampler` (and its warmed `System`) is process-global, shared across
connections; only the push task is per-connection — so the CPU delta survives
reconnects. As written the ownership boundary is still implicit. Low impact (a
single zeroed sample per reconnect at worst).

### OBSERVATIONS / NON-ISSUES (re-verified, no action)

- `WorkerMessage` enum tagging
  `#[serde(tag = "type", rename_all = "snake_case")]` — confirmed `ws.rs:130`;
  wire tag `"metrics"` correct.
- `ServerMessage::Welcome { protocol_version: u32, connection_id, .. }` —
  confirmed `ws.rs:56`. Adding `#[serde(default)] accepts_metrics: bool` is
  additive; the absent-⇒-`false` reasoning (lines 156-166) is correct, and the
  bare-`bool`-not-`Option` argument citing the `BuildRevoke.reason` precedent
  (`ws.rs:48-52`) is sound.
- Server hard-rejects `protocol_version != 2` — confirmed
  `cbsd-server/src/ws/handler.rs:141`; riding `accepts_metrics` inside v2 as an
  additive field needs no version bump.
- `OUTPUT_CHANNEL_CAPACITY = 64` — confirmed `cbsd-worker/src/ws/handler.rs:50`;
  `out_tx` per-connection, cloned to the supervisor — confirmed
  `ws/handler.rs:91-92`. The per-connection sampler holding its own
  `out_tx.clone()` (lines 260-265) is the right shape.
- `send_or_spool` drops non-output messages when no build owns the transport —
  confirmed `cbsd-worker/src/build/supervisor.rs:264`/`:678`. Bypassing it for
  metrics (lines 271-278) is correct: idle-worker host stats must not be
  dropped.
- `classify_exit_code(Option<i32>) -> BuildFinishedStatus` with
  Success/Revoked(137,143)/Failure — confirmed
  `cbsd-worker/src/build/executor.rs:275`. Reusing it for `subprocess_exits`
  (lines 245-248) maps cleanly to the success/failure/revoked triple.
- `try_send` drop-on-`Full` + `push_drops_total` counter (lines 273-277, 252) —
  correct backpressure: never `send().await`, never route through the
  supervisor.
- `default_true`/`default_push_interval`/`default_ccache_interval` are net-new
  helper fns (the codebase has only explicit `default_*` fns today, no
  `default_true`). Trivial; the design implies they exist but this is a
  no-impact note.

## Cross-document consistency

- **Schema → exposition coverage (021 → 022): clean, re-confirmed post-edit.**
  Every field in `HostMetrics` / `AppMetrics` / `CcacheMetrics` /
  `SubprocessExitCounts` maps to a 022 family and vice-versa, with no orphans
  either direction: `uptime_secs` → `cbsd_worker_uptime_seconds`;
  `filesystems[].{mount,total,used}` →
  `cbsd_worker_host_fs_{total,used}_bytes {mount}`;
  `subprocess_exits.{success,failure,revoked}` →
  `cbsd_worker_subprocess_exits_total{result}`; `push_drops_total` →
  `cbsd_worker_metrics_push_drops_total`; `spool_bytes` →
  `cbsd_worker_spool_bytes`. Genuine strength.
- **Label vocabulary now consistent.** `result` is used for the
  success/failure/revoked classification in both 021's `subprocess_exits`
  republish and 022's `cbsd_build_results_total` /
  `cbsd_worker_subprocess_exits_total`. `_total`/`_bytes`/`_seconds` suffix
  conventions hold across the schema. No drift remains.

## Confidence score

| Item                                                               | Points | Description                                                                |
| ------------------------------------------------------------------ | ------ | -------------------------------------------------------------------------- |
| Starting score                                                     | 100    |                                                                            |
| D4: m1 obsolete `/proc` fallback hedge at current `sysinfo` 0.39.x | -5     | Non-idiomatic / stale-version guidance (softened from v1 but still there)  |
| D8: m2 per-connection sampler vs warmed CPU `System` ownership     | -5     | Reconnect zeroes one CPU sample unless `System` ownership is made explicit |
| **Total**                                                          | **90** |                                                                            |

**Interpretation: 90 / 100 — ready to merge; minor or no issues.** **Delta vs
v1: +30 (60 → 90).** The v1 dominant deductions (M1's false parity claim and the
hidden net-new `WorkerMessageTag` work, −25 combined; the `Counter::absolute`
notation, −5) are all resolved. Only two low-impact carry-over minors remain,
neither blocking.

### Most important remaining fix

State explicitly that the warmed `HostSampler`/`sysinfo::System` is
process-global and outlives the per-connection push task (m2), so the
two-refresh CPU delta is not zeroed on every reconnect. Everything else is
optional polish.
