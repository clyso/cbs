# Plan Review: cbsd-rs Implementation Plans v2 (Phases 0–6)

**Plans reviewed:**
- `cbsd-rs/docs/cbsd-rs/plans/README.md` through `002-20260318T1411-05-integration.md` (all 9 files)

**Cross-referenced against:**
- `cbsd-rs/docs/cbsd-rs/design/README.md` and all 3 design documents

---

## Summary

The plans have been updated to incorporate prior feedback: the `active`/`workers` struct split between Phases 3 and 4 is now explicit, the Commit 8 split into 8a/8b is done, pre-accept revoke handling is correctly placed, `grace_period_secs` is in the `welcome` message, `build_logs` row insertion is under the dispatch mutex, and the PASETO cross-language test is tracked. The plans are well-structured and largely ready to implement.

Three blockers remain. The most immediately impactful is B1: the `cargo sqlx prepare` bootstrap procedure in Commit 2 is circular (compile-time query macros require the offline cache, but generating the cache requires a compilable binary) with no step-by-step resolution provided — the implementer will hit this wall on day one. B2 (`trace_id` is generated after the mutex is released and never persisted — a crash loses it permanently with no recovery path) is a debuggability gap that should be resolved at schema design time. B3 (drain-mode shutdown flushes logs in the wrong position relative to WS close) risks data loss on decommission.

**Verdict: Approve with conditions.** Fix B1–B3. Address the 5 significant concerns in-place before the relevant commits. Phases 0 and 1 can proceed immediately.

---

## Blockers

### B1 — `cargo sqlx prepare` bootstrap in Commit 2 is circular

Commit 2 says "Run `cargo sqlx prepare` and commit `.sqlx/` directory." But `cargo sqlx prepare` requires a compilable binary (for query analysis), and the binary uses `sqlx::query!` macros that require either a live database or the `.sqlx/` cache to compile. Neither exists when Commit 2 is first being written.

**Fix:** Add a numbered "sqlx offline cache bootstrap" procedure to Commit 2:
1. Write migration SQL files.
2. Create dev DB: `sqlx database create` + `sqlx migrate run` (requires `DATABASE_URL` env var).
3. Write all `db/*.rs` query code with `DATABASE_URL` pointing to the live dev DB.
4. Run `cargo sqlx prepare --workspace` from workspace root.
5. Verify: `SQLX_OFFLINE=true cargo build --workspace` succeeds.
6. Commit `.sqlx/` directory as a separate "chore: add sqlx offline query cache" commit.
7. Specify where `.sqlx/` lives (workspace root or per-crate).

### B2 — `trace_id` generated outside mutex, never persisted — crash loses it permanently

Commit 8a generates `trace_id` (UUID v4) after the dispatch mutex is released. The `builds` table has no `trace_id` column. If the server crashes between mutex release and WS send, the build is marked FAILURE by startup recovery — but the `trace_id` is gone. Operators cannot correlate server-side and worker-side logs for that failed build.

**Fix:** Either (a) add `trace_id TEXT` to the `builds` table in Commit 2's migration, generate it under the mutex, persist alongside the DISPATCHED state write — this is the right choice for production debuggability. Or (b) add an explicit note: "`trace_id` is ephemeral and not persisted — correlation is only possible for builds that reached a connected worker." If (a), re-run `cargo sqlx prepare` (links to B1).

### B3 — Drain-mode shutdown log flush in wrong position

Commit 13's SIGQUIT/`--drain` sequence: (1) stop accepting, (2) send `build_revoke`, (3) wait for acks, (4) mark unacknowledged FAILURE, (5) "close WS connections, flush logs, shut down."

During the drain wait (step 3), workers send `build_output` and `build_finished` messages. The log writer is processing these asynchronously. If WS connections are closed (step 5) and the server exits before in-flight log writes complete, the last output lines are silently dropped.

**Fix:** Revise Commit 13's drain sequence to: (5a) close WS connections, (5b) drain log writer's pending write queue (bounded timeout or completion signal), (5c) flush `build_logs.log_size`/`finished` metadata to SQLite, (5d) shut down. Document the invariant: "no build output is dropped after WS close."

---

## Major Concerns

### M1 — `ack_timer_handle` cancellation mechanism unspecified

Commit 7 adds `ack_timer_handle` to `ActiveBuild`. Commit 8a cancels it on `build_accepted`. The plan doesn't specify the mechanism — `JoinHandle::abort()`, `CancellationToken`, or `tokio::select!` with a `Sleep` future. The choice affects correctness: if the timer fires after the build transitions to STARTED (race between abort and timer expiry), a spurious re-queue occurs.

**Fix:** Specify: "The ack timer is a `tokio::time::Sleep` future polled inside the WS connection task via `tokio::select!`. Cancellation is modeled as dropping the `Sleep` future (or using a `CancellationToken` stored in `ActiveBuild` under the dispatch mutex)."

### M2 — Commit 5 AND-semantics boundary test case missing

Commit 5 implements `require_scopes_all` with assignment-level AND semantics. Commit 6 tests scope checks on build submission. But if the AND logic is wrong, Commit 6's integration test could pass against a naive OR implementation.

**Fix:** Add an explicit test case to Commit 5's testable section: "two assignments, each covering a different scope type → build submission rejected with 403, even though both scope types are individually satisfied by different assignments."

### M3 — Commit 12 recovery wiring into app.rs unnamed

Commit 12 implements startup recovery in `queue/recovery.rs` but doesn't name the call site. Without wiring it into `app.rs` lifespan (after migrations, before accepting connections), the recovery code exists but is never called, and the testable items silently fail.

**Fix:** Add: "Wire `queue::recovery::run_startup_recovery()` into `app.rs` lifespan, called after migrations complete and before the server begins accepting HTTP connections or WebSocket upgrades."

### M4 — `DELETE /api/builds/{id}` for QUEUED state has a TOCTOU window

Commit 6's QUEUED revocation: "remove from queue, mark REVOKED, return 200." The plan doesn't specify that queue removal and DB update happen atomically under the `SharedBuildQueue` mutex. Without this, a concurrent dispatch can pop the build between the state check and the removal, leaving the build DISPATCHED in memory but REVOKED in the DB.

**Fix:** Add to Commit 6: "QUEUED revocation: acquire `SharedBuildQueue` mutex → search lanes for build_id → if found, remove from lane + update DB to REVOKED under mutex → return 200. If not found (race: already dispatched), fall through to DISPATCHED handling (deferred to Phase 4)."

### M5 — Server-side backoff ceiling validation missing from Commit 2

The design says: "The server validates `reconnect_backoff_ceiling < grace_period` at startup and refuses to start if violated." Commit 10 handles the worker side. But Commit 2's config validation only mentions `allowed_domains`/`allow_any_google_account`. An operator with `ceiling=120, grace=90` starts a broken server.

**Fix:** Add to Commit 2 config validation: "Panic at startup if `reconnect_backoff_ceiling_secs >= liveness_grace_period_secs`."

---

## Minor Issues

- **`connection_id` and `trace_id` in `cbsd-proto` must be `String`, not `Uuid`.** The proto crate has no `uuid` dep. Server generates UUIDs internally and serializes to string.
- **OAuth flow test is not automatable in CI.** Commit 4's "Full OAuth flow with test Google project" requires external credentials. Clarify as manual integration test; automated tests should mock the token endpoint.
- **`GET /api/admin/queue` should read from in-memory queue, not DB.** The DB shows stale DISPATCHED state during the dispatch critical section. Specify: read from `SharedBuildQueue` under a short lock.
- **seq→offset Vec sort invariant undocumented.** Binary search assumes monotonically increasing insertion order. Add assertion in `logs/writer.rs`.
- **`pre_exec` async-signal-safety note needed.** Only async-signal-safe functions (`setsid`) are allowed between `fork()` and `exec()`. No logging or allocations in the closure.
- **`cron`/`tokio-cron-scheduler` should not be added in v1.** GC uses `tokio::time::interval`. Note explicitly that cron crates are deferred.
- **`BuildId` newtype must wrap `i64`.** SQLite `INTEGER PRIMARY KEY AUTOINCREMENT` maps to `i64` in sqlx.
- **Argon2 on cache miss should use `spawn_blocking`.** Inline argon2 (~100–500ms) blocks a tokio worker thread.
- **Commit 8a: `uuid` crate dependency missing from project structure.** `trace_id` generation requires it in `cbsd-server`.
- **Structured result line detection: specify prefix-based.** `line.starts_with(r#"{"type":"result""#)` before full JSON parse. Don't parse every output line.
- **GC interval should not fire immediately on startup.** A restart at 03:01 after a 03:00 start would GC twice. Start the interval after the first `log_retention_days` tick.
- **Seeded API keys in container logs.** Plaintext is printed to stdout, which podman/docker captures. Note that operators must retrieve keys from container logs before rotation.
- **`WorkerState` enum variants unspecified.** The plan mentions the worker state machine but never defines the enum. Commit 7 should list variants (`Connected`, `Disconnected`, `Stopping`, `Dead`) and which are dispatch-eligible.
- **Python wrapper script location unspecified.** Commit 11 describes what it does but not where it lives in the repo or whether it's part of the worker container image build.
- **GC task `JoinHandle` should be stored in `AppState`.** A detached `tokio::spawn` can't be cleanly shut down. Store the handle and await it during shutdown.
- **Last-admin guard: 5 test cases, not 1.** Commit 5's testable section says "prevents removing sole admin through all 5 paths" — list them as 5 named test cases.

---

## Suggestions

- **Create `cbsd-server/CLAUDE.md`** listing the three easy-to-miss correctness invariants: (a) dispatch mutex held across SQLite write, released before WS send, (b) `max_connections = 4` pool sizing to prevent deadlock, (c) `foreign_keys=ON` per-connection pragma.
- **Embed the reconnection decision table as a Rust comment block** above the `worker_status` handler in `ws/handler.rs`. Makes the 10-row table self-documenting at the implementation site.
- **Name `app.rs` as the canonical `AppState` location** in Commit 2 and note "subsequent commits extend this struct." Prevents struct definition from drifting across files.
- **Specify bootstrapping key output format:** `worker-01: cbsk_...` for operator correlation.
- **Add a `trace_id` note to CLAUDE.md** (Phase 0): "trace_id is generated at dispatch time and propagated to the worker for cross-boundary log correlation."
- **Specify the `.sqlx/` directory relative path** in the project structure doc and CLAUDE.md so CI configuration is unambiguous.

---

## Strengths

- **Commit 8a/8b split.** Happy-path dispatch tested independently from revocation/reconnection. Exactly the right decomposition.
- **`active`/`workers` ownership boundary explicit.** Commit 6 owns queue lanes; Commit 7 adds `active` + `workers`. Clean, documented, testable at each boundary.
- **`build_logs` row inserted under dispatch mutex (Commit 8a).** Matches the design's "write to SQLite under mutex before WS send" invariant. Prevents crash-gap bug.
- **Worker backoff ceiling clamping (Commit 10).** Clamp to `grace_period_secs - 10s` with warning log. Exactly the defensive behavior the design requires.
- **Seeding transaction ordering (Commit 12).** Commit transaction first, then print keys. Prevents orphaned printed keys on rollback.
- **PASETO cross-language test in Commit 3.** Hardcoded expected bytes, not emergent field ordering. Directly addresses the canonical form requirement.
- **Phase 5 parallelism note.** Commit 10 infra can be developed alongside Phase 4. Exploitable for parallel development.
- **Design–plan authority chain clear.** "Design docs win over plans" — correct posture.

---

## Open Questions

1. **Where does `.sqlx/` live?** Workspace root (`cbsd-rs/.sqlx/`) or per-crate (`cbsd-rs/cbsd-server/.sqlx/`)? Determines the exact `cargo sqlx prepare` invocation.
2. **Is `trace_id` persisted in the `builds` table?** If yes, add column to Commit 2 migration. If no, document the consequence.
3. **What constitutes "idle" for dispatch?** `WorkerState` variants are undefined. Which states are dispatch-eligible?
4. **Who creates the Python wrapper script?** Where does it live? Is it committed to the repo or baked into the container image?
5. **Does `CachedApiKey` include `key_prefix`?** Required for LRU eviction cleanup. Must be confirmed before Commit 4.
6. **Commit 13 GC task handle: stored in `AppState` or detached spawn?** Affects clean shutdown.
