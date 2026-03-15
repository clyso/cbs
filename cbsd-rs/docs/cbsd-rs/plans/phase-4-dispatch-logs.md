# Phase 4 — Dispatch & Logs: WebSocket, Dispatch, State Machine, Log Streaming

## Progress

| Item | Status |
|------|--------|
| Commit 7: WebSocket handler — hello/welcome, liveness, worker tracking | Done |
| Commit 8a: Build dispatch — happy path (dispatch → accept → start → finish) | Done |
| Commit 8b: Revocation, reconnection decision table, periodic sweep | Done |
| Commit 9: Log writer, SSE streaming, component tarball packing | Done |

## Goal

Server accepts worker WebSocket connections, dispatches builds to workers,
tracks the full build lifecycle (DISPATCHED→STARTED→SUCCESS/FAILURE/REVOKED),
streams logs via SSE.

## Depends on

Phase 3 (build queue must exist to dispatch from).

## Commit 7: WebSocket handler — hello/welcome, liveness, worker tracking

The server-side WebSocket infrastructure. Also extends `BuildQueue` with the
`active` map and worker registry.

**ws/handler.rs:**
- `WS /api/ws/worker` — axum WebSocket upgrade handler. Authenticates via
  `Authorization: Bearer <api-key>` header in the HTTP upgrade request
  (reuses `AuthUser` extractor). Unauthenticated → 401, never upgraded.
- Per-connection message loop: receives `hello` (validates protocol version,
  arch against known enum), sends `welcome` (with `connection_id` UUID and
  `grace_period_secs` from config). Dispatches all `WorkerMessage` variants
  to appropriate handlers.

**queue/mod.rs (extend):**
- Add `active: HashMap<BuildId, ActiveBuild>` and
  `workers: HashMap<ConnectionId, WorkerState>` to `BuildQueue`.
- `ActiveBuild` struct: `build_id`, `connection_id`, `dispatched_at`,
  `trace_id`, `descriptor` (for component packing), `ack_cancel`
  (`CancellationToken` for ack timer).
- `WorkerState` enum variants: `Connected` (dispatch-eligible when no entry
  in `active` map for this connection), `Disconnected { since: Instant }`,
  `Stopping`, `Dead`. Only `Connected` workers with no active build are
  dispatch-eligible.
- Worker registry keyed by server-assigned UUID (not `worker_id` string).

**ws/liveness.rs:**
- Worker state machine: Connected → Disconnected(since) → Dead (on grace
  period expiry). Connected → Stopping (on `worker_stopping`) → Dead
  (immediate on WS drop, no grace period; any DISPATCHED build re-queued).
- Grace period timer (configurable, default 90s).

**Module call direction:** `ws/handler.rs` owns the per-connection WS loop
and calls into `ws/dispatch.rs` (Commit 8a) and `ws/liveness.rs`. The
dispatch module exposes functions, not a task — no circular deps.

**routes/workers.rs:**
- `GET /api/workers` — lists connected workers with connection_id,
  worker_id label, arch, state, current build_id.

**Testable:** Mock WS client can connect, complete hello/welcome handshake
(including `grace_period_secs` in welcome). Worker registry tracks
connections. Liveness states transition correctly. `GET /api/workers` returns
connected workers.

## Commit 8a: Build dispatch — happy path (dispatch → accept → start → finish)

The dispatch engine for the normal build lifecycle. Split from 8b to keep
each commit under ~400 lines with independent testability.

**ws/dispatch.rs:**
- Split-mutex dispatch: under lock → generate `trace_id` (UUID v4) → pop
  queue + mark DISPATCHED + persist `trace_id` to `builds.trace_id` + insert
  `build_logs` row (`log_path`, `log_size=0`, `finished=0`) + create
  `watch::channel()` sender and store in `AppState.log_watchers:
  HashMap<BuildId, watch::Sender<()>>` (separate from `ActiveBuild`, avoids
  coupling queue to log subsystem) — all under mutex as a single SQLite
  transaction. Release lock → pack component tarball + send `build_new` JSON
  + binary frame. On send failure → re-acquire lock, push to front of lane,
  remove watch sender.
- **Ack timer mechanism:** The ack timeout is a `tokio::time::Sleep` future
  in a `tokio::select!` branch within the WS connection task. Cancellation
  is via a `CancellationToken` stored in `ActiveBuild` under the mutex.
  `build_accepted` triggers `ack_cancel.cancel()`, dropping the Sleep branch.
- Ack timeout (configurable, default 15s): on expiry → re-acquire lock,
  re-queue at front.
- `build_accepted` → cancels ack timer, build stays DISPATCHED.
  `build_started` → transition to STARTED.
- `build_finished` → transition to SUCCESS or FAILURE. Drop watch sender
  from `AppState.log_watchers`. Worker becomes idle, triggers re-dispatch.
- Handles `build_rejected`: if integrity failure → mark FAILURE (not
  re-queue); otherwise → re-queue at front, try next worker.
- Worker selection (v1): first idle worker with matching arch.

**components/tarball.rs:**
- Pack component directory → tar.gz bytes + SHA-256 hash.
- No caching (re-pack per dispatch).

**Build state transitions (happy path):**
- QUEUED → DISPATCHED (dispatch), DISPATCHED → STARTED (`build_started`),
  STARTED → SUCCESS/FAILURE (`build_finished`).
- DISPATCHED → QUEUED (ack timeout, send failure, reject).

**Testable:** Build dispatched to connected worker. Ack timeout re-queues.
`build_accepted` → `build_started` → `build_finished(success)` transitions
correctly. Integrity failure → FAILURE (not re-queue). Component tarball
packing works.

## Commit 8b: Revocation, reconnection decision table, periodic sweep

The error/edge-case handling that completes the dispatch engine.

**ws/dispatch.rs (extend):**
- `DELETE /api/builds/{id}` extended: DISPATCHED/STARTED → send
  `build_revoke`, transition to REVOKING, return 202. Revoking → 200 no-op.
  Terminal → 409.
- Revoke ack timeout (configurable, default 30s): on expiry → mark REVOKED
  unilaterally. Late `build_output` after `finished=1` discarded.
- `build_revoke` before `build_accepted`: worker responds with
  `build_finished(revoked)` immediately — server handles this by
  transitioning DISPATCHED → REVOKED (or REVOKING → REVOKED).

**Reconnection decision table** (in ws/handler.rs, triggered on
`worker_status` after reconnect hello):
- Full 10-row table from design doc.
- Grace period expiry table: DISPATCHED/STARTED→FAILURE, REVOKING→REVOKED.

**Re-dispatch periodic sweep:**
- 30-second `tokio::time::interval`. `JoinHandle` stored in `AppState` for
  clean shutdown (not a detached `tokio::spawn`). Checks for QUEUED builds
  with no active dispatch attempt. Catches edge cases (worker reconnection
  without build completion, missed dispatch triggers).

**Testable:** Build revocation flow (STARTED → REVOKING → REVOKED).
Revoke ack timeout marks REVOKED unilaterally. Pre-accept revoke handled.
Reconnection decision table: worker reconnects mid-build → resume or revoke
per table. Grace period expiry → FAILURE. 30-second sweep dispatches
orphaned QUEUED builds. **`worker_stopping` mid-dispatch test:** ack timeout
fires while worker is `Stopping` → build re-queued, worker state remains
`Stopping` (not marked suspect).

## Commit 9: Log writer, SSE streaming, build log endpoints

Build log management and streaming.

**logs/writer.rs:**
- Append each `build_output` message to per-build log file
  (`{log_dir}/builds/{build_id}.log`) before processing next message.
- Per-line seq tracking: `start_seq + index` for each line in batch.
- In-memory seq→offset index: `HashMap<BuildId, Vec<(u64, u64)>>`. Inserted
  per-line on append. Dropped when build terminal.
- `tokio::sync::watch` channel per active build: wakeup signal (single-slot,
  coalesced — handler reads from current file position to EOF on wakeup, not
  just to the watch value offset).
- `build_logs.log_size` updated every 5 seconds or on completion.
  `build_logs.finished = 1` set atomically on `build_finished`.

**logs/sse.rs:**
- `GET /api/builds/{id}/logs/follow` — SSE (`text/event-stream`).
  `event: output` with `id: <per_line_seq>`. `event: done` on `finished=1`.
- Resumption: `Last-Event-ID` header → binary search in seq→offset index →
  seek to exact byte offset. O(log n).
- FD held open for stream lifetime (design constraint: prevents GC race on
  Linux via open-inode survival).
- Missing file for terminal build → synthetic `event: done`.
- Completed builds: no watch channel, linear scan fallback (acceptable, rare).

**routes/builds.rs (extend):**
- `GET /api/builds/{id}/logs/tail?n=30` — last N lines, cap n ≤ 10000.
- `GET /api/builds/{id}/logs/follow` — wired to SSE handler.
- `GET /api/builds/{id}/logs` — full log file download (streaming response).

**Testable:** Build output arrives → log file written → SSE streams to client.
SSE resume from `Last-Event-ID` produces correct lines (no duplicates, exact
per-line seq). Tail returns last N lines. Full download works. Watch channel
wakeup delivers output with sub-millisecond latency.
