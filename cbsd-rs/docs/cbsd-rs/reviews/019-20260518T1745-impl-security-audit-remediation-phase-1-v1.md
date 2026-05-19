---
seq: "019"
timestamp: "20260518T1745"
type: impl
title: security-audit-remediation-phase-1
version: v1
design: >
  019-20260516T0715-design-security-audit-remediation-v8.md (WCP design v11:
  019-20260426T1154-worker-control-plane-hardening.md)
plan: 019-20260518T0554-plan-security-audit-remediation-v3.md
commits: wip/cbsd-rs-security-review (Phase 1 — commits 1–6)
---

# Implementation Review — WCP Security Audit Remediation Phase 1

**Reviewer:** independent adversarial review\
**Date:** 2026-05-18\
**Score:** 20 / 100 — **No-Go. Fix three blockers before Phase 2.**

---

## 1. Summary Assessment

Phase 1 delivers five of its six objectives correctly and without shortcuts: the
six-column provenance rollback is exact, the receipt state machine is correctly
wired, the two-phase ownership check is lock-safe, the component descriptor
validator covers all four ingress paths, the log-tail bound is enforced and
tested, and the worker supervisor is correctly scoped at process level. The Rust
idioms throughout are sound (no `.unwrap()` outside tests, no blocking in async,
no `std::sync::Mutex` held across `.await`).

Three findings make this a no-go for Phase 2, each of which leaves a real
invariant broken:

1. The send-failure rollback path in `try_dispatch` rolls back memory but not
   the database, violating WCP D4 SI-6/SI-13.
2. `WorkerStatus` is absent from `WorkerBuildAction`, so every unauthorized
   `worker_status(building)` message is reported on the wire as `BuildStarted` —
   a false action code.
3. `idle_reconcile_decision` returns `Skip` for `"revoking"` state when WCP D3
   requires `Revoked`, and a unit test cements the wrong behavior.

None of these are speculative edge-case risks. Finding 1 leaves a live build
stuck `dispatched` in the database after a retry; finding 3 leaves a `revoking`
build unreachable by the idle reconciler every time a worker reconnects idle.

---

## 2. Scope and Evidence Basis

All claims below are grounded in direct source reads from the branch
`wip/cbsd-rs-security-review`, not in implementer documentation. Primary files
reviewed:

- `cbsd-proto/src/ws.rs`
- `cbsd-server/src/db/builds.rs`
- `cbsd-server/src/ws/dispatch.rs`
- `cbsd-server/src/ws/handler.rs`
- `cbsd-server/src/queue/mod.rs`
- `cbsd-server/src/components/validator.rs`
- `cbsd-server/src/logs/tail.rs`
- `cbsd-worker/src/build/supervisor.rs`
- `cbsd-worker/src/ws/handler.rs`
- `cbsd-worker/src/main.rs`

Design authority: WCP design v11
(`019-20260426T1154-worker-control-plane-hardening.md`). Plan authority: v3
(`019-20260518T0554-plan-security-audit-remediation-v3.md`).

---

## 3. Per-Commit Assessment

### Commit 1 — `rollback_dispatch_to_queued` DB helper

**Plan scope:** Dedicated six-column rollback function in
`cbsd-server/src/db/builds.rs`; two unit tests; ban on using
`update_build_state` for queued rollback.

**Verdict: Pass.**

`rollback_dispatch_to_queued` (line 252) issues one `UPDATE` statement clearing
all six columns (`worker_id`, `trace_id`, `error`, `started_at`, `finished_at`,
`build_report`) atomically. The doc-comment explicitly names WCP D4 SI-6 and
SI-13 and explains why `update_build_state` is the wrong tool. Two tests cover
the happy path and the unknown-id case. `update_build_state` remains in the
codebase but the only callsite outside tests is `routes/builds.rs:366` for the
`revoked` terminal transition — not for queued rollback.

**LOC:** plan ~320, actual ~537 (+68%). Over-budget but the surplus is
doc-comments and a more thorough test setup; the commit still constitutes a
single coherent, independently compilable unit.

---

### Commit 2 — `ActiveAssignmentReceipt` and ack-timer wiring

**Plan scope:** `ActiveAssignmentReceipt` enum in `cbsd-server` (not
`cbsd-proto`); `ActiveBuild.receipt` field; `authorize_- lifecycle_message`
atomically sets `ReceivedByWorker` + cancels ack-cancel under queue lock;
`handle_build_accepted` cancels timer idempotently.

**Verdict: Pass.**

`ActiveAssignmentReceipt` is in `cbsd-server/src/queue/mod.rs` lines 32–57,
correctly scoped inside the server crate per plan note N1.
`authorize_lifecycle_message` (dispatch.rs line 534) acquires the queue lock
once, sets `ReceivedByWorker`, and calls `ack_cancel.cancel()` — all under the
same lock acquisition. `handle_build_accepted` (dispatch.rs line 278) calls
`ack_cancel.cancel()` again; this is idempotent and correct since the authorize
gate already updated the receipt state before this handler runs.

**LOC:** plan ~720, actual ~534 (−26%). Under target but the commit is complete
and compilable.

---

### Commit 3 — Idle reconnect reconciliation

**Plan scope:** `idle_reconcile_decision` pure function with 4-cell matrix;
`"revoking"` + idle → `Revoked`; unit tests for all matrix cells;
`rollback_active_to_queued` helper called by idle reconciler.

**Verdict: Partial fail — one cell wrong, test pins wrong behavior.**

`idle_reconcile_decision` (handler.rs lines 722–736) correctly handles:

- `"dispatched"` + `AwaitingReceipt` + `prev_connection_live` → `Skip`
- `"dispatched"` + anything else → `RollbackToQueued`
- `"started"` → `FailBuild`

But the wildcard arm (`_ =>`) returns `Skip`, so `"revoking"` maps to `Skip`.
WCP D3 explicit matrix row: `revoking + idle worker → revoked`. Plan commit 3
pitfall warning names this exact cell. The unit test at line 1241
(`idle_other_states_are_skipped`) includes `"revoking"` in the set of states
asserted to yield `Skip`, cementing the incorrect behavior as a passing test.

This is a correctness blocker: a build in `revoking` state can never be resolved
by idle reconciliation. It requires a `"revoking"` arm in the match that returns
`IdleReconcileAction:: Revoked` (or equivalent), and the test must be corrected.

**LOC:** plan ~450, actual ~274 (−39%). Under target partly because the
`Revoked` arm is missing.

---

### Commit 4 — Component descriptor validation at dispatch

**Plan scope:** `validate_descriptor` in
`cbsd-server/src/ components/validator.rs`; wired at all four ingress paths
(manual dispatch, periodic trigger, reconnect-Building claim, test- dispatch
route); validator errors → `TriggerError::Fatal` → `disable_with_error`.

**Verdict: Pass.**

`validate_descriptor` (validator.rs) checks empty component list and unknown
component names, returning a typed `ValidationError`. The scheduler trigger
(trigger.rs lines 79–84) maps `ValidationError` → `TriggerError::Fatal`, which
scheduler/mod.rs line 262 routes to `disable_with_error`. All four ingress paths
confirmed by grep. Old `validate_component_name` function removed. Four unit
tests cover expected cases.

**LOC:** plan ~180, actual ~183. On target.

---

### Commit 5 — Bounded log tail

**Plan scope:** `read_tail` in `cbsd-server/src/logs/tail.rs` with configurable
byte budget (4 MiB in routes/builds.rs); UTF-8 safe; `truncated` set only on
budget hit; six unit tests.

**Verdict: Pass.**

`read_tail` (tail.rs) seeks to `max(0, file_size - max_bytes)`, reads forward,
and sets `truncated = truncated_start` (line 129), where `truncated_start` is
true only when the seek landed at a non-zero offset.
`MAX_TAIL_BYTES = 4 * 1024 * 1024` is defined in `routes/builds.rs`. Boundary
conditions are correct: a single line exceeding the budget returns empty with
`truncated=true`; a one-liner with no newline returns empty. Six unit tests
cover all significant edge cases. No `.unwrap()` outside tests. Default for the
`-n` flag in `cbc/src/logs.rs` is `50`.

**LOC:** plan ~330, actual ~276 (−16%). Complete and correct.

---

### Commit 6 — Worker supervisor and reconnect ownership

**Plan scope:** Process-level `Arc<Supervisor>` in `main.rs`;
`take_reconnect_messages` correct ordering (Building → spool → terminal);
`UnauthorizedBuildAction` arm in worker handler; `reporter_directed_revoke` arm;
send-failure rollback via `rollback_active_to_queued` at server `try_dispatch`
step 12.

**Verdict: Partial fail — two issues.**

**Issue A (Blocker — send-failure rollback gap):** `try_dispatch` step 12
(dispatch.rs lines 206–230) does not call `rollback_active_to_queued`. It
manually removes the build from `queue.active`, re-enqueues in memory, and
removes the watcher — but never calls `rollback_dispatch_to_queued`. The
database retains `state='dispatched'` with the stale `worker_id` and `trace_id`
from the failed attempt. This violates WCP D4 SI-6 and SI-13 and means the
entire rationale for commit 1 (the dedicated rollback function) is bypassed by
the most common rollback trigger. The plan's commit-6 pitfall section names this
exact hazard, yet the implementation omits the DB call.

**Issue B (Blocker — `WorkerStatus` absent from `WorkerBuildAction`):**
`WorkerBuildAction` (ws.rs lines 69–75) has five variants: `BuildAccepted`,
`BuildStarted`, `BuildOutput`, `BuildFinished`, `BuildRejected`. The
`WorkerStatus` variant is absent. The handler (handler.rs line 804) reports
unauthorized `worker_status(building)` to the worker as
`WorkerBuildAction::BuildStarted` — a factually incorrect action code on the
wire. WCP D3 defines `WorkerStatus` as a first-class `WorkerBuildAction`. Plan
note N3 records this requirement. This is a protocol correctness failure.

**Good in commit 6:**

- `Arc<Supervisor>` is created at `main.rs` line 153, before `reconnect_loop`,
  and passed as `Arc::clone` — correctly process-level.
- `take_reconnect_messages` (supervisor.rs lines 327–383) produces messages in
  the correct order: Building claim first, spool content second, terminal state
  last.
- `UnauthorizedBuildAction` arm in `cbsd-worker/src/ws/handler.rs` (lines
  192–207) logs `build_id`, `action`, and `reason` at warn. This arm is
  preserved through the commit 6 rewrite and the match is exhaustive.
- `reporter_directed_revoke` arm handled.
- No `.unwrap()` outside tests, no `std::sync::Mutex`, no blocking calls in
  async context.

**LOC:** plan ~750, actual significantly over (commit 6 touches ~1 523 lines
across files; net new ~1 160). The overrun reflects the scope of the supervisor
implementation; it does not indicate padding.

---

## 4. Cross-Commit Invariant Verification

### INV-1: DB rollback always clears all six provenance columns

**Status: Violated (commit 6 send-failure path).**

`rollback_dispatch_to_queued` (commit 1) is correct in isolation.
`rollback_active_to_queued` (commit 6, dispatch.rs line 345) calls it correctly.
But the send-failure branch in `try_dispatch` (commit 6, lines 213–228) does not
call either function. The DB invariant holds on the ack-timeout path and the
idle reconcile path; it is broken on the send-failure path.

### INV-2: `ActiveAssignmentReceipt` transitions are under queue lock

**Status: Preserved.**

Both the `AwaitingReceipt` → `ReceivedByWorker` transition (authorize gate) and
the ack-cancel call are performed inside a single `queue.lock().await`
acquisition. No transition occurs outside the lock.

### INV-3: `UnauthorizedBuildAction` wire message survives commit 6 rewrite

**Status: Preserved.**

The worker handler match arm added in commit 2 is present in the commit 6
version of `cbsd-worker/src/ws/handler.rs` lines 192–207. The match is
exhaustive. The cross-commit invariant holds.

### INV-4: Queue lock is never held across SQLite I/O

**Status: Preserved on all rollback paths verified.**

`rollback_active_to_queued` releases the queue lock before calling
`rollback_dispatch_to_queued` (lines 334–337 acquire, 339–341 release, 345 DB
call). The two-phase ownership check in `handle_worker_status` (handler.rs line
758 DB call outside lock, line 845 lock reacquired) also preserves this
invariant.

### INV-5: `revoking` state is handled by idle reconciler

**Status: Violated (commit 3).**

`idle_reconcile_decision` wildcards `"revoking"` to `Skip`. The design matrix
requires `Revoked`. The unit test at line 1241 locks in the wrong behavior. A
`revoking` build will never be moved to `revoked` by the idle reconciler; it
requires manual intervention or a subsequent worker reconnect.

---

## 5. Findings Register

### BLOCKER — F1: Send-failure rollback does not clear DB

**Location:** `cbsd-server/src/ws/dispatch.rs` lines 206–230\
**Design ref:** WCP D4, SI-6, SI-13\
**Plan ref:** Commit 6, Step 12, pitfall warning

After a failed WebSocket frame enqueue in `try_dispatch`, the code removes the
build from `queue.active` and re-enqueues it in memory, but does not call
`rollback_dispatch_to_queued`. The database row retains `state='dispatched'`
with the stale `worker_id` and `trace_id` from the failed attempt. When the
build is subsequently dispatched to a second worker, the old `worker_id` is
still in the DB at dispatch time. When the ack-timeout fires or an idle
reconciler runs, it will see stale provenance.

**Fix:** Replace the manual `queue.active.remove` + `enqueue_front` block with a
call to `rollback_active_to_queued`, or add a
`rollback_dispatch_to_queued(pool, build_id).await` call before the re-enqueue.
The `rollback_active_to_queued` helper already exists and does exactly this; use
it.

---

### BLOCKER — F2: `WorkerStatus` absent from `WorkerBuildAction`

**Location:** `cbsd-proto/src/ws.rs` lines 69–75\
**Design ref:** WCP D3, `WorkerBuildAction` enum at
`019-20260426T1154-worker-control-plane-hardening.md` line 247 — `WorkerStatus`
is listed as the **first** normative variant\
**Plan ref:** Commit 2 note N3; commit 3 unauthorized-status path

The `WorkerBuildAction` enum has five variants; `WorkerStatus` is missing. The
handler reports unauthorized `worker_status(building)` messages to the worker as
`BuildStarted`. This produces a false action name on the wire, misleading
operator logs and potentially confusing future automated tooling that parses
`UnauthorizedBuildAction` records.

**Fix:** Add `WorkerStatus` to `WorkerBuildAction` in `cbsd-proto/src/ws.rs`.
Update the handler callsite (handler.rs line 804) to use
`WorkerBuildAction::WorkerStatus`. Run `cargo sqlx prepare --workspace` is not
required (proto change only); `cargo test --workspace` must pass.

---

### BLOCKER — F3: `revoking` incorrectly skipped by idle reconciler

**Location:** `cbsd-server/src/ws/handler.rs` lines 727–736 (function), line
1241 (test)\
**Design ref:** WCP D3, idle-worker decision table row:
`revoking + idle → revoked`\
**Plan ref:** Commit 3, implementation note and pitfall section

The wildcard arm of `idle_reconcile_decision` returns `Skip` for all states not
explicitly matched, including `"revoking"`. WCP D3 requires that a `revoking`
build meeting an idle worker be transitioned to `revoked`. The unit test at line
1241 includes `"revoking"` among the states that must return `Skip`, cementing
the wrong behavior as a passing assertion.

**Fix:**

1. Add `"revoking" => IdleReconcileAction::Revoked` (or
   `IdleReconcileAction::RevokeAndComplete` per final enum naming) before the
   wildcard arm.
2. Remove `"revoking"` from the `idle_other_states_are_skipped` test array.
3. Add a dedicated test: `idle_revoking_dispatched_to_revoked`.
4. Implement the `Revoked` arm in the reconciler call-site that dispatches on
   `IdleReconcileAction`.

---

### MAJOR — F4: No integration tests for idle reconcile or

reconnect-Building ownership

**Location:** `cbsd-server/src/ws/handler.rs` — no `#[tokio::test]` functions
exercising the full reconcile loop\
**Design ref:** WCP D3 reconnect ownership matrix\
**Plan ref:** Commit 3 test list, commit 6 test list

The idle reconcile and reconnect-Building ownership paths are exercised only by
pure unit tests on the decision functions (`idle_reconcile_decision`,
`reconnect_building_decision`). No integration test drives a WebSocket
`worker_status(idle)` message through `AppState` and verifies that the DB row
transitions correctly. Given that the idle reconciler has a confirmed
correctness bug (F3), this gap leaves future regressions undetected at the
unit-test level — the broken `"revoking"` behavior is currently confirmed
correct by a unit test.

**Recommendation:** Add at least two integration tests: one for
`dispatched + idle + AwaitingReceipt` → queued rollback, one for
`revoking + idle` → revoked. These should use a real SQLite pool and a mock
`worker_senders` map.

---

### MINOR — F5: Queue lock held across `drain_spool` I/O in supervisor

**Location:** `cbsd-worker/src/build/supervisor.rs` lines 346, 360\
**Design ref:** CLAUDE.md correctness invariant #2 (async mutex and I/O)\
**Severity:** Minor — the mutex is a `tokio::sync::Mutex`, so there is no
deadlock risk, but holding it across the `drain_spool().await` call serializes
all supervisor operations for the duration of the spool read.

**Recommendation:** Read the spool content before acquiring the lock, or extract
the spool data under the lock and then drain outside it.

---

## 6. What Is Correctly Deferred

The following items are explicitly Phase 3+ scope and are not findings:

- Receipt-aware `handle_worker_dead` cleanup path (commit 20)
- `last_authenticated_connect_at` timestamp (commit 21)
- `BuildRevoke` reason field (commit 21)
- Reporter-directed revoke reason propagation to worker logs

These are noted and correctly absent.

---

## 7. Confidence Score

| Item                                                    | Points       | Description                                                            |
| ------------------------------------------------------- | ------------ | ---------------------------------------------------------------------- |
| Starting score                                          | 100          |                                                                        |
| D7: send-failure path skips DB rollback                 | −20          | WCP D4 SI-6/SI-13 violated; stale provenance persists in DB            |
| D7: `WorkerStatus` absent, wrong action on wire         | −20          | Normative enum variant missing (WCP D3 line 247); false action on wire |
| D8: `revoking` → `Skip` violates WCP D3 matrix          | −5           | Spec deviation — correct cell is `Revoked`                             |
| D5: unit test pins wrong `revoking` behavior            | −15          | Test asserts `Skip` for `revoking`; bug is green at CI                 |
| D5: no integration test for full idle reconcile         | −15          | Critical reconnect paths untested end-to-end                           |
| D9: send-failure logs error but DB divergence is silent | −5           | Error log says "failed to send"; nothing warns DB was not rolled back  |
| **Total**                                               | **80**       |                                                                        |
| **Score**                                               | **20 / 100** |                                                                        |

---

## 8. Recommendation

**No-Go. Phase 2 must not begin until F1, F2, and F3 are fixed.**

Required actions before proceeding:

1. **F1 (send-failure rollback):** In `try_dispatch` step 12, replace the manual
   rollback with a call to `rollback_active_to_queued`. Add a unit test that
   verifies the DB row returns to `state='queued'` with all six columns NULL
   after a simulated send failure.

2. **F2 (`WorkerStatus` enum):** Add `WorkerStatus` to `WorkerBuildAction` in
   `cbsd-proto`. Update the single callsite in handler.rs line 804. Existing
   serialization tests will catch any wire-format regression.

3. **F3 (revoking idle reconcile):** Add the `"revoking"` arm to
   `idle_reconcile_decision`, remove it from the skip test, add a dedicated
   test. Implement the `Revoked` dispatch arm in the reconciler.

F4 (integration tests) and F5 (supervisor lock scope) are non-blocking for Phase
2 but should be addressed before Phase 3 adds more reconnect logic on top of the
untested paths.
