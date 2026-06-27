# Phase: Metrics — Wire Protocol & Worker Collector

**Design document:**
`cbsd-rs/docs/cbsd-rs/design/021-20260626T0942-metrics-protocol-and-worker-collector.md`

Companion plans: `022-…-metrics-server-exporter-and-aggregation.md` (server),
`023-…-metrics-deployment-and-dashboards.md` (ops). The global build order and
dependency graph live in the 022 plan.

## Progress

| #   | Commit                                                        | ~LOC | Status  |
| --- | ------------------------------------------------------------- | ---- | ------- |
| 1   | `cbsd-rs/proto: add metrics wire types and accepts_metrics`   | ~350 | Pending |
| 2   | `cbsd-rs/worker: collect and push metrics over the WebSocket` | ~650 | Pending |

**Total:** ~1000 LOC, 2 commits.

This plan owns global build steps **G4** (commit 1) and **G6** (commit 2). G4 is
independent and unblocks the server ingestion commit (022#5 / G5). G6 compiles
on G4 alone and is end-to-end visible once G5 has landed.

---

## Commit 1 (G4): cbsd-proto metrics wire types

**Delivers:** the wire format both ends agree on, fully serde-tested; a
`WorkerMessageTag` mirror so future `WorkerMessage` variants fail the build
until handled. No production caller yet — the serde/parity tests are the readers
(pub types in a lib crate, no dead-code warnings). First production consumers
are 022#5 (server) and this plan's commit 2 (worker).

**Files:**

- `cbsd-rs/cbsd-proto/src/ws.rs` (new `WorkerMessage::Metrics` variant; the
  `HostMetrics` / `FilesystemUsage` / `AppMetrics` / `CcacheMetrics` /
  `SubprocessExitCounts` structs; `Welcome.accepts_metrics`; `WorkerMessageTag`
  enum + `from_message` + parity test)

**Steps:**

1. Add `Metrics { uptime_secs, host, app }` to `WorkerMessage`
   (`#[serde(tag = "type", rename_all = "snake_case")]` → wire tag `"metrics"`).
   Add the payload structs exactly as in design §"Wire protocol additions"
   (`mem_available_bytes`, swap, per-mount `filesystems`, cumulative disk IO;
   `app`: `ccache: Option<…>`, `subprocess_exits`, `spool_bytes`,
   `push_drops_total`).
2. Add `accepts_metrics` to `ServerMessage::Welcome` as a bare
   `#[serde(default)] bool` (NOT `Option<bool>`) — absent ⇒ `false` ⇒ silent. No
   `protocol_version` bump (server still hard-rejects `!= 2`).
3. Add `WorkerMessageTag` (enum + `from_message` mapping + parity/exhaustiveness
   test) mirroring `ServerMessageTag` (`ws.rs:581`, `:596`, `:717`) — design m4.

**Tests:**

- `metrics` round-trips: `WorkerMessage::Metrics { .. }` → JSON (tag
  `"metrics"`) → back, value-equal.
- Missing-field compat (N1): a `Welcome` JSON without `accepts_metrics`
  deserializes with `accepts_metrics == false`.
- Tag-parity test covers the new `Metrics` variant.
- An old peer ignores unknown variants/fields (keep the existing
  no-`deny_unknown_fields` assertion green).

---

## Commit 2 (G6): worker collector and per-connection push

**Delivers:** a connected worker samples host + app metrics and pushes them over
the existing WebSocket when the server advertises `accepts_metrics`;
backpressure and sampler lifecycle are tested. End-to-end visible once 022#5
(G5) is in.

**Files:**

- `cbsd-rs/cbsd-worker/Cargo.toml` (add `sysinfo`)
- `cbsd-rs/cbsd-worker/src/config.rs` (new `WorkerMetricsConfig`; resolve onto
  `ResolvedWorkerConfig`)
- `cbsd-rs/cbsd-worker/src/metrics/host.rs` (new — process-global `HostSampler`)
- `cbsd-rs/cbsd-worker/src/metrics/app.rs` (new — ccache, subprocess-exit
  counters, spool, push-drops)
- `cbsd-rs/cbsd-worker/src/metrics/sampler.rs` (new — per-connection task)
- `cbsd-rs/cbsd-worker/src/build/executor.rs` (increment subprocess-exit
  counters at result finalization)
- `cbsd-rs/cbsd-worker/src/build/supervisor.rs` (expose the current spool-fill
  byte count for `spool_bytes`; the tracker is the `spool_bytes` field
  (`supervisor.rs:92-93`) under the 64 MiB `DEFAULT_SPOOL_CAP_BYTES` budget,
  read behind the supervisor's async state lock)
- `cbsd-rs/cbsd-worker/src/ws/handler.rs` (read `Welcome.accepts_metrics`; spawn
  the sampler task with its own `out_tx.clone()`)

**Steps:**

1. `WorkerMetricsConfig` (kebab-case, `#[serde(default)]` parent): `enabled`
   (default true), `push_interval_secs` (15), `ccache_interval_secs` (60). No
   bind — the worker exposes no endpoint.
2. `HostSampler` wraps one long-lived `sysinfo::System`, **process-global**
   behind a `Mutex` so CPU-delta state survives reconnects. Produces a
   `HostMetrics` per call (cpu_busy_ratio, load1, mem/swap, per-mount
   filesystems, disk IO). First CPU sample zeroed. Disk IO uses the
   **cumulative** `Disk::usage().total_read_bytes` / `total_written_bytes`
   summed across disks (NOT the per-refresh `read_bytes`/`written_bytes` deltas)
   — these are republished as counters server-side, so the wire value must be
   monotonic-since-boot, not a per-tick delta.
3. App sources, all **process-global** (survive reconnects, reset only on
   worker-process restart): ccache via `ccache -s` on the slower
   `ccache_interval_secs` cadence, **carried forward and attached to every
   push** (load-bearing for gauge freshness — design §App-metric sources);
   `SubprocessExitCounts` as an `AtomicU64` trio incremented in the executor;
   `spool_bytes` from the supervisor's budget tracker; `push_drops_total`
   `AtomicU64` bumped on `try_send` `Full`.
4. Per-connection sampler task: spawned in `run_connection` only if
   `metrics.enabled` **and** `Welcome.accepts_metrics`; holds its own
   `out_tx.clone()` (sibling of the supervisor's clone). Each tick builds a
   `WorkerMessage::Metrics` and `out_tx.try_send(..)`; on `Full`, increment
   `push_drops_total` and drop (never `send().await`). Never routed through the
   supervisor (which drops idle-time messages). Cancelled on connection drop;
   reconnect spawns a fresh sampler. Channel is `OUTPUT_CHANNEL_CAPACITY = 64`.

**Tests:**

- `try_send` on a saturated 64-cap channel increments `push_drops_total` and
  does not block.
- Sampler is not spawned when `accepts_metrics == false` or
  `metrics.enabled == false`.
- Sampler task terminates when its connection's `out_rx` is dropped.
- `HostSampler` returns a plausible non-empty snapshot on the second sample
  (first-sample CPU zeroing honored).
- Subprocess-exit counters increment per classified result.

---

## Implementation notes

- `sysinfo` pinned at **0.33** during implementation (the estimate here was ≈
  0.39.x; 0.33 was the resolved version and carries the required disk-IO fields
  `DiskUsage::total_read_bytes` / `total_written_bytes` as first-class on
  Linux). No field had to be omitted for lack of a source.
- ccache stats parsing format varies across ccache versions — pin to the worker
  image's version at implementation.
- Default sampled mounts (scratch, ccache, container storage) are derived from
  existing worker config paths; finalize at implementation. </content>
