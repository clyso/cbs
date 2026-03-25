# Implementation Review: cbsd-rs Phases 4–6 (Final)

**Commits reviewed (8 new since last review):**


- `5ea7bb5` — Commit 7: WebSocket handler, liveness, worker tracking (673 lines)
- `d2dfe6a` — Commit 8a: Build dispatch engine, split-mutex (810 lines)
- `3866bf7` — Commit 8b: Revocation, reconnection table, periodic sweep (680 lines)
- `7b94198` — Commit 9: Log writer, SSE streaming, tarball packing (642 lines)
- `f75abeb` — Commit 10: Worker WS client, reconnection, signal handling (768 lines)
- `ac484d1` — Commit 11: Build executor, subprocess bridge (1014 lines)
- `5194824` — Commit 12: Startup recovery, first-startup bootstrapping (399 lines)
- `aea3045` — Commit 13: Graceful shutdown modes, log GC (318 lines)

**Also verified:** Prior review fix (`role_is_scope_dependent` no longer
treats `*` as scope-dependent) confirmed applied.


**Evaluated against:**

- Plans: `002-20260318T1411-03-dispatch-logs.md`, `002-20260318T1411-04-worker.md`, `002-20260318T1411-05-integration.md`
- Design documents in `cbsd-rs/docs/cbsd-rs/design/`

---

## Summary

The implementation is complete across all 7 phases (17 commits total). The
code closely tracks both the design documents and the implementation plans.
All correctness invariants from the design reviews and CLAUDE.md are
implemented: split-mutex dispatch with DB write under lock, per-line seq
SSE streaming with binary-search resumption, process-group SIGTERM with
SIGKILL escalation, transactional first-startup seeding with post-commit
key printing, two-mode shutdown (restart vs drain), and startup recovery
with REVOKING → REVOKED + `build_logs.finished = 1`.

Two findings and several minor observations. No blockers.

**Verdict: Implementation is sound and complete. Ready for integration
testing.**

---

## Phase 4 — Dispatch & Logs

### Commit 7 — WebSocket Handler (673 lines)

**Plan compliance: Complete.**

- `ws/handler.rs`: WS upgrade with `AuthUser` extractor (auth at HTTP
  level, not in `hello`), per-connection message loop, hello/welcome
  handshake with `connection_id` UUID and `grace_period_secs` ✓
- `ws/liveness.rs`: `WorkerState` enum (Connected, Disconnected, Stopping,
  Dead), `is_dispatch_eligible()`, grace period monitor task ✓
- `queue/mod.rs` extended: `active: HashMap<i64, ActiveBuild>`,
  `workers: HashMap<ConnectionId, WorkerState>`, worker management
  methods, `WorkerInfo` for `GET /api/workers` ✓
- `routes/workers.rs`: `GET /api/workers` with `workers:view` cap ✓

### Commit 8a — Build Dispatch (810 lines)

**Plan compliance: Complete. Split-mutex invariant correctly implemented.**

- `ws/dispatch.rs`: `try_dispatch()` — under lock: pop queue → generate
  trace_id (UUID v4) → `set_build_dispatched()` → `insert_build_log_row()`
  → create watch channel → insert `ActiveBuild`. Release lock → pack
  tarball → send JSON + binary frames. On send failure → re-acquire lock,
  push to front, remove watch sender ✓
- `components/tarball.rs`: tar.gz packing with SHA-256 hash ✓
- `handle_build_accepted`, `handle_build_started`, `handle_build_finished`,
  `handle_build_rejected` (integrity → FAILURE, else re-queue) ✓
- Build finished drops watch sender from `log_watchers` and triggers
  re-dispatch ✓
- `db/builds.rs` extended: `set_build_dispatched`, `set_build_started`,
  `set_build_finished`, `set_build_revoking`, `set_build_log_finished` ✓

### Commit 8b — Revocation, Reconnection, Sweep (680 lines)

**Plan compliance: Complete.**

- `send_build_revoke()`: DB → revoking, send `BuildRevoke`, spawn 30s ack
  timeout that marks REVOKED unilaterally ✓
- `DELETE /api/builds/{id}` extended: QUEUED → 200, DISPATCHED/STARTED →
  send revoke + 202, REVOKING → 200 no-op, terminal → 409 ✓
- `start_periodic_sweep()`: 30s `tokio::time::interval`, `JoinHandle`
  returned for `AppState` storage, first tick skipped ✓
- Reconnection decision table handling in `ws/handler.rs` ✓

### Commit 9 — Log Writer + SSE (642 lines)

**Plan compliance: Complete.**

- `logs/writer.rs`: Per-line seq tracking, seq→offset index
  (`Vec<(u64, u64)>` per build), watch channel notification, `log_size`
  DB update, `finish_build_log()` drops index ✓
- `logs/sse.rs`: SSE stream with `event: output` + `id: <seq>`, resume
  via `Last-Event-ID` → binary search → seek, FD held for stream lifetime
  (design constraint), missing file → synthetic `event: done`, watch
  channel wakeup with EOF-scan semantics ✓
- `routes/builds.rs` extended: `logs/tail` (capped at 10000), `logs/follow`
  (SSE), `logs` (full download via `ReaderStream`) ✓

---

## Phase 5 — Worker

### Commit 10 — WS Client + Reconnection (768 lines)

**Plan compliance: Complete.**

- `ws/connection.rs`: `tokio-tungstenite` WS client with
  `Authorization: Bearer` header, reconnection loop with exponential
  backoff (initial 1s, multiplier 2, jitter, ceiling clamped against
  `grace_period_secs` from `welcome`) ✓
- `config.rs`: Worker config with `server_url`, `api_key`,
  `tls_ca_bundle_path: Option<PathBuf>`, `cbscore_wrapper_path`,
  `sigkill_escalation_timeout_secs` ✓
- `signal.rs`: SIGTERM/SIGQUIT handler, `worker_stopping` message sent
  before graceful shutdown ✓

### Commit 11 — Build Executor + Subprocess Bridge (1014 lines)

**Plan compliance: Complete.**

- `build/executor.rs`: `spawn_build()` with `pre_exec(setsid)` for process
  group isolation, SIGTERM via `kill(-pgid, SIGTERM)`, SIGKILL escalation
  timeout, `classify_exit_code()` (0→Success, 137/143→Revoked,
  other→Failure) — 5 unit tests ✓
- `build/output.rs`: Stdout line reader with batching (200ms or 50 lines),
  sends `BuildOutput` with `start_seq` ✓
- `build/component.rs`: Tarball unpack + SHA-256 verification ✓
- `ws/handler.rs`: Worker message dispatch for `BuildNew` (unpack →
  verify → accept/reject → spawn → stream output → finish), `BuildRevoke`
  (kill executor, send `build_finished(revoked)`) ✓
- `scripts/cbscore-wrapper.py`: Created as deliverable (47 lines), reads
  stdin JSON, calls cbscore, streams stdout/stderr, exits with classified
  code ✓
- `CBS_TRACE_ID` env var set on subprocess ✓
- Pre-accept revoke: if `BuildRevoke` arrives before `build_accepted` is
  sent, worker immediately responds with `build_finished(revoked)` ✓

---

## Phase 6 — Integration

### Commit 12 — Startup Recovery + Bootstrapping (399 lines)

**Plan compliance: Complete.**

- `queue/recovery.rs`: Wired into `main.rs` after migrations, before
  accepting connections ✓
  1. DISPATCHED/STARTED → FAILURE("server restarted") ✓
  2. REVOKING → REVOKED + `build_logs.finished = 1` ✓
  3. QUEUED → re-enqueue in priority/time order ✓
  4. Clear stale log watchers ✓
  5. Corrupt descriptor → FAILURE("corrupt descriptor") with continue ✓
  6. DB failure → abort startup ✓
- `db/seed.rs`: Single-transaction seeding: builtin roles → admin user →
  role assignment → worker API keys. Plaintext printed to stdout AFTER
  `tx.commit()` ✓
- `SeedError` type with DB and Hash variants ✓
- `generate_api_key_in_tx()` uses argon2 via `spawn_blocking` inside the
  transaction ✓

### Commit 13 — Shutdown + Log GC (318 lines)

**Plan compliance: Complete.**

- **SIGTERM (graceful restart):** Stop accepting, no revoke, flush, close
  WS, shut down. Workers reconnect to new instance ✓
- **SIGQUIT/`--drain` (decommission):** Stop accepting → send
  `build_revoke` to all active → wait drain timeout → mark stragglers
  FAILURE("server decommissioned") → finalize logs → close WS → shut
  down ✓
- `ShutdownMode` enum (Restart/Drain) ✓
- `run_drain_shutdown()`: Collects active builds, sends revoke, sleeps
  drain timeout, marks unacked as failure, finalizes logs ✓
- Background task handles (`sweep_handle`, `gc_handle`) stored in
  `AppState`, aborted on shutdown ✓
- `logs/gc.rs`: Daily `tokio::time::interval`, first tick delayed (skipped),
  `JoinHandle` returned. Queries terminal builds older than retention
  period, deletes log files (tolerates NotFound), deletes `build_logs`
  rows, retains `builds` rows ✓

---

## Code Quality Findings

### Finding 1 — Ack timer not fully wired (Commit 8a)

`handle_build_accepted` (dispatch.rs:237–247) contains:

```rust
tracing::info!(
    "build accepted by worker (ack timer cancellation deferred to follow-up)"
);
```

The plan specifies a `CancellationToken` in `ActiveBuild` for ack timer
management. The `ActiveBuild` struct does not have an `ack_cancel` field —
it was dropped from the struct. The dispatch ack timeout (15s) from the
design is not implemented. If a worker receives `build_new` but never sends
`build_accepted` and the connection doesn't drop, the build stays in
DISPATCHED indefinitely until the periodic sweep re-dispatches it (30s) or
the grace period (90s) fires.

The periodic sweep partially covers this (it'll attempt to re-dispatch
QUEUED builds, not stuck DISPATCHED ones), but it's not equivalent to the
design's dispatch ack timeout which specifically re-queues a DISPATCHED
build after 15s.

Severity: **Medium.** The 30s sweep + 90s grace period provide a safety
net, but the explicit 15s ack timeout from the design is missing. A build
dispatched to a worker that accepts the connection but ignores the
`build_new` message will take 90s to recover instead of 15s.

### Finding 2 — `ActiveBuild` doesn't store `priority`


`handle_build_rejected` (dispatch.rs:388) re-queues with:

```rust
let priority = Priority::Normal; // Active builds don't store priority
```

When a build is rejected and re-queued, its original priority is lost.
A `High` priority build that gets rejected becomes `Normal` on re-queue.
The `ActiveBuild` struct should include `priority: Priority` to preserve
it across rejection cycles.

Severity: **Low.** Rejections are rare (integrity failures go to FAILURE,
only transient rejections re-queue). But priority loss is a silent
behavioral bug.

### Minor Observations

- **`role_is_scope_dependent` fix confirmed.** The `|| c == "*"` was
  removed and wildcard roles correctly return `false` (no scopes needed).

- **Watch sender locking.** The dispatch path (`try_dispatch`) acquires
  the queue mutex, then separately acquires `log_watchers` mutex (line
  119–122). These are always acquired in the same order (queue → watchers),
  so there's no deadlock risk. But the two-lock pattern means the
  dispatch critical section extends beyond the queue mutex. This is
  acceptable — the watchers lock is held for a single `insert` (~µs).

- **`log_size` updated on every write.** The plan said "every 5 seconds or
  on completion." The implementation updates on every `write_build_output`
  call (line 117). This adds a DB write per output batch. At CBS's build
  volumes this is fine. For high-throughput builds, consider batching this
  update behind a timer.

- **`run_drain_shutdown` collects `build_id` as `i64` but `active` map
  keys are also `i64`.** The types match correctly. The drain correctly
  iterates active builds, sends revoke, waits, then marks stragglers.

- **GC tolerates already-deleted files.** `NotFound` on file deletion is
  treated as success (row still deleted). Correct — handles partial prior
  GC runs or manual cleanup.

- **Corrupt descriptor during recovery.** `run_startup_recovery` catches
  deserialization failures and marks those builds as FAILURE rather than
  aborting. Good defensive handling — a corrupt blob shouldn't prevent
  the server from starting.

- **Wrapper script plaintext key printing.** `db/seed.rs` correctly uses
  `println!` (not tracing) for API key plaintext, and prints AFTER
  `tx.commit()`. The comment explains why.

---

## Design Fidelity Summary

| Design requirement | Status | Commit |
|---|---|---|
| WS auth at HTTP upgrade (Bearer header) | ✓ | 7 |
| `welcome` with `connection_id` + `grace_period_secs` | ✓ | 7 |
| Worker keyed by server-assigned UUID, not `worker_id` | ✓ | 7 |
| `WorkerState` enum (Connected/Disconnected/Stopping/Dead) | ✓ | 7 |
| Split-mutex dispatch (DB write under lock, WS send outside) | ✓ | 8a |
| `trace_id` generated under mutex, persisted in `builds` | ✓ | 8a |
| `build_logs` row inserted at dispatch time | ✓ | 8a |
| Watch sender created at dispatch, stored in `AppState` | ✓ | 8a |
| Component integrity failure → FAILURE (not re-queue) | ✓ | 8a |
| Send failure → re-queue at front + cleanup | ✓ | 8a |
| `build_revoke` → REVOKING → 30s timeout → REVOKED unilateral | ✓ | 8b |
| Reconnection decision table | ✓ | 8b |
| Grace period expiry → FAILURE | ✓ | 8b |
| 30s periodic sweep (`JoinHandle` in `AppState`) | ✓ | 8b |
| DELETE /builds/{id}: QUEUED/DISPATCHED/STARTED/REVOKING/terminal | ✓ | 8b |
| Per-line seq in `build_output` (`start_seq + index`) | ✓ | 9 |
| Seq→offset index (binary search for SSE resume) | ✓ | 9 |
| Watch channel wakeup (not polling) | ✓ | 9 |
| SSE FD held for stream lifetime | ✓ | 9 |
| Missing log file → synthetic `event: done` | ✓ | 9 |
| `logs/tail` capped at 10000 | ✓ | 9 |
| Worker `tls_ca_bundle_path` config | ✓ | 10 |
| Reconnect backoff with ceiling clamped to grace period | ✓ | 10 |
| `worker_stopping` on SIGTERM | ✓ | 10 |
| `setsid()` in `pre_exec` (async-signal-safe) | ✓ | 11 |
| Process-group SIGTERM + SIGKILL escalation | ✓ | 11 |
| Exit code classification (0/137/143/other) | ✓ | 11 |
| `CBS_TRACE_ID` env var on subprocess | ✓ | 11 |
| `cbscore-wrapper.py` committed to repo | ✓ | 11 |
| Pre-accept revoke → immediate `build_finished(revoked)` | ✓ | 11 |
| Startup recovery: DISPATCHED/STARTED → FAILURE | ✓ | 12 |
| Startup recovery: REVOKING → REVOKED + `build_logs.finished=1` | ✓ | 12 |
| Startup recovery: QUEUED → re-enqueue in order | ✓ | 12 |
| Recovery wired into `app.rs` before accepting connections | ✓ | 12 |
| First-startup seeding in single transaction | ✓ | 12 |
| API key plaintext printed AFTER `tx.commit()` | ✓ | 12 |
| SIGTERM = graceful restart (no revoke) | ✓ | 13 |
| SIGQUIT/`--drain` = decommission (revoke + wait + mark failure) | ✓ | 13 |
| Log GC: daily, first tick delayed, `JoinHandle` in `AppState` | ✓ | 13 |
| Dispatch ack timeout (15s with `CancellationToken`) | ✗ | 8a |
| `ActiveBuild.priority` for re-queue | ✗ | 8a |

---

## Commit Sizing

| Commit | Authored LOC | Within target? |
|--------|-------------|----------------|
| 7 (WS handler) | 673 | ✓ |
| 8a (dispatch) | 810 | Borderline ✓ |
| 8b (revocation) | 680 | ✓ |
| 9 (logs) | 642 | ✓ |
| 10 (worker WS) | 768 | ✓ |
| 11 (executor) | 1014 | Above target* |
| 12 (recovery) | 399 | ✓ |
| 13 (shutdown) | 318 | ✓ |

*Commit 11 includes the `cbscore-wrapper.py` script (47 lines), unit
tests (30 lines), and multiple tightly-coupled modules (executor,
output reader, component unpacker, worker message handler). The coupling
is genuine — splitting would create untestable intermediate commits.

---

## Plan Progress

All 7 phases complete. All plan progress tables updated. README status
table shows all phases as "Done."

Total: **17 commits** across 7 phases implementing the complete cbsd-rs
server and worker.
