# 021 — Review: Metrics Wire Protocol & Worker Collector (v1)

## Verdict

**Conditional go.** The wire-protocol design is sound and matches the codebase
on every structural claim (enum tagging, `Welcome` shape, `BuildRevoke` serde
precedent, additive `#[serde(default)] bool`, protocol-version handling). One
**major** factual error must be fixed before implementation: the document
asserts an existing tag-parity test will automatically enforce the new
`WorkerMessage::Metrics` variant — no such test exists for `WorkerMessage`. Two
crate-API notation/version items are minor but worth correcting. Cross-document
field coverage with 022 is clean (no orphan fields).

Reviewed against the actual source at commit on branch `wip/cbsd-rs-metrics` and
against current stable crate docs on docs.rs.

## Findings

### MAJOR

**M1 — The claimed tag-parity test does not exist for `WorkerMessage`.** Lines
130-133 ("Tag table") state: _"`ws.rs` maintains a `WorkerMessageTag` enum
mirrored against the variants (used by the strict-parse round-trip test,
~`ws.rs:585+`/`:717`). Add a `Metrics` tag there too; the existing test that
asserts tag↔variant parity will enforce it."_

This is false. The tag/parity machinery in `cbsd-proto/src/ws.rs` is
**`ServerMessage`-only**:

- `ServerMessageTag` enum: `ws.rs:581-588` (there is **no** `WorkerMessageTag`).
- `from_message(&ServerMessage)` compile-forced witness: `ws.rs:596`.
- `sentinel_for_tag(ServerMessageTag)`: `ws.rs:623`.
- the test `no_deny_unknown_fields_on_server_message`: `ws.rs:717`, iterating
  `ServerMessageTag::iter()` (`ws.rs:725`).

`WorkerMessage` (`ws.rs:130`) has only **per-variant round-trip tests**
(`worker_message_hello_round_trip` at `ws.rs:274`, `…build_finished…`,
`…worker_status…`, etc.) and **no exhaustiveness/parity test**. Adding
`WorkerMessage::Metrics` would compile and ship with zero test coverage unless a
test is written by hand; nothing "enforces it." The design's own "Required serde
tests" section (lines 174-180) does ask for a `metrics` round-trip test and a
"tag-parity test [that] covers the new `Metrics` variant" — but that parity test
**does not exist and would have to be created** (likely by introducing a
`WorkerMessageTag` + witness mirroring the `ServerMessage` pattern). Fix: delete
the false "existing test … will enforce it" claim, and re-scope line 178 to "add
a `WorkerMessageTag` + exhaustiveness test mirroring `ServerMessageTag`"
(net-new work), or accept per-variant round-trip coverage only and say so
explicitly.

### MINOR

**m1 — `Counter::absolute()` notation is misleading (forward ref to 022).** Line
137 says counters are republished _"verbatim via `Counter::absolute()`"_. In the
`metrics` facade (v0.24.x) `absolute` is an **instance method** (`&self`), not
an associated function; the idiomatic call is
`counter!("name", labels).absolute(v)`
(https://docs.rs/metrics/latest/metrics/struct.Counter.html). Writing
`Counter::absolute()` reads like a free/associated fn and will mislead the
implementer. Same wording recurs in 022; fix in both. (This belongs to 022's
plane but is asserted here, so noted in both reviews.)

**m2 — `sysinfo` `/proc` fallback hedge is likely obsolete at current
versions.** Lines 224-226 hedge that `disk_*_bytes_total` and `load1` "coverage
varies by `sysinfo` release … fall back to reading `/proc`". For `load1`,
`sysinfo::System::load_average()` returns `LoadAvg { one, five, fifteen }` and
is available on Linux — no fallback needed. For host-wide cumulative disk IO,
current `sysinfo` (0.36+; latest stable 0.39.x) exposes
`Disk::usage() -> DiskUsage { total_read_bytes, total_written_bytes, … }`,
summable across `Disks`, so `/proc/diskstats` is also unnecessary at the
pinned-latest version
(https://docs.rs/sysinfo/latest/sysinfo/struct.DiskUsage.html). The "verify
against the pinned version" instruction is correct and saves the design, but the
hedge as written understates current capability. Pin a concrete version and drop
the `/proc` plan if it lands on a recent `sysinfo`.

**m3 — First-sample CPU zeroing interacts with a per-connection sampler
lifetime.** Lines 218-219/228 correctly note `sysinfo` needs two refreshes for a
CPU delta and that "the first sample after start is discarded/zeroed." Because
the sampler is **per-connection** (lines 251-257, correctly mirroring `out_tx`
at `ws/handler.rs:91-92`), every reconnect restarts the sampler task. If the
`HostSampler`/`System` is re-instantiated per connection, the first push after
each reconnect carries a zeroed `cpu_busy_ratio`. The design says the `System`
is "single long-lived" (line 213) — clarify that the `HostSampler` (and its
warmed `System`) is **process-global and shared across connections**, while only
the _push task_ is per-connection, so the CPU delta survives reconnects. As
written the ownership of the `System` vs the task is ambiguous.

### OBSERVATIONS / NON-ISSUES (verified, no action)

- `WorkerMessage` enum tagging
  `#[serde(tag = "type", rename_all = "snake_case")]` — **confirmed**
  `ws.rs:128-130`; wire tag `"metrics"` is correct.
- `ServerMessage::Welcome` fields `protocol_version: u32`,
  `connection_id: String`, `grace_period_secs: u64` — **confirmed**
  `ws.rs:54-60`. Adding `#[serde(default) accepts_metrics: bool]` is additive
  and the missing-field-⇒-`false` reasoning (lines 156-161) is correct.
- `BuildRevoke.reason` precedent
  `#[serde(default, skip_serializing_if = "Option::is_none")] reason: Option<BuildRevokeReason>`
  — **confirmed** `ws.rs:48-52`. The design's argument that `accepts_metrics`
  should be a bare `bool` (not `Option<bool>`) is sound.
- Server hard-rejects `protocol_version != 2` — **confirmed**
  `cbsd-server/src/ws/handler.rs:141`; riding `accepts_metrics` inside v2 as
  additive is correct (no version bump needed).
- `OUTPUT_CHANNEL_CAPACITY = 64` — **confirmed** `cbsd-worker/src/ws/handler.rs`
  (const at the top of the module); `out_tx` per-connection cloned to supervisor
  — **confirmed** `ws/handler.rs:91-92`.
- `send_or_spool` drops non-output messages when no build owns the transport —
  **confirmed** `cbsd-worker/src/build/supervisor.rs` (~269-275 path into the
  drop at ~273; helper at ~678). Bypassing it for metrics (lines 268-270) is the
  correct call: idle-worker host stats must not be dropped.
- `classify_exit_code(Option<i32>) -> BuildFinishedStatus` with
  Success/Revoked(137,143)/Failure — **confirmed**
  `cbsd-worker/src/build/executor.rs:275`. Reusing it for `subprocess_exits`
  (lines 237-240) maps cleanly to the `success/failure/revoked` triple.
- Worker config pattern (kebab-case rename, a `#[serde(default)]` parent
  section, `serde_saphyr::from_str`) — **confirmed** in
  `cbsd-worker/src/config.rs`. NOTE: the codebase has **no `default_true` helper
  today** (server/worker use explicit `default_*` fns like `default_log_level`);
  the design assumes `default_true` (line 196) and
  `default_push_interval`/`default_ccache_interval` — these are net-new helper
  fns the implementer must add. Trivial, but the design implies they exist.

## Cross-document consistency

- **Schema → exposition coverage (021 → 022): clean.** Every field in
  `HostMetrics` / `AppMetrics` / `CcacheMetrics` / `SubprocessExitCounts` has a
  corresponding family in 022's plane-2 catalog, and 022 exposes nothing absent
  from the 021 schema. `uptime_secs` → `cbsd_worker_uptime_seconds`;
  `filesystems[].{mount,total,used}` →
  `cbsd_worker_host_fs_{total,used}_bytes {mount}`; `push_drops_total` →
  `cbsd_worker_metrics_push_drops_total`. No orphans either direction. This is a
  genuine strength.
- **Label-name drift `outcome` vs `result`.** 021 line 102 republishes
  subprocess exits "with an `outcome` label"; 022 confirms
  `cbsd_worker_subprocess_exits_total{outcome}`. But the server-owned build
  outcome counter is `cbsd_build_results_total{result}` (022 line 136). The same
  success/failure/revoked classification is labeled `outcome` in one family and
  `result` in another. Not breaking, but it forces dashboard authors to remember
  which is which. Consider standardizing on one (`result` is already the
  dominant server-side choice). Flagged again in 022's review.

## Confidence score

| Item                                                            | Points | Description                                                                        |
| --------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------- |
| Starting score                                                  | 100    |                                                                                    |
| D8: M1 false "existing parity test enforces it" claim           | -5     | Spec/architecture deviation: the enforcing test does not exist for `WorkerMessage` |
| D1: M1 net-new `WorkerMessageTag`+exhaustiveness test unscoped  | -20    | Required-but-unspecified work presented as already-covered                         |
| D8: m1 `Counter::absolute()` notation wrong (facade is `&self`) | -5     | Misleading API notation                                                            |
| D4: m2 obsolete `/proc` fallback hedge at current `sysinfo`     | -5     | Non-idiomatic / stale-version guidance                                             |
| D8: m3 per-connection sampler vs warmed CPU `System` ambiguity  | -5     | Reconnect zeroes CPU unless `System` ownership clarified                           |
| **Total**                                                       | **60** |                                                                                    |

**Interpretation: 60 / 100 — significant issues; address before proceeding.**
The dominant deduction is M1 (the parity-test claim is wrong _and_ it hides
net-new test/infra work). Everything else is minor and the structural protocol
design is correct. With M1 reworded/rescoped and the three minors corrected,
this rises comfortably into the 85+ band.

### Most important fix

Correct lines 130-133 (and 178): there is **no** existing tag-parity test for
`WorkerMessage`. State that adding `WorkerMessage::Metrics` requires a net-new
`WorkerMessageTag` + exhaustiveness test mirroring the `ServerMessage` pattern
(`ws.rs:581-765`), or explicitly accept per-variant round-trip coverage only.
