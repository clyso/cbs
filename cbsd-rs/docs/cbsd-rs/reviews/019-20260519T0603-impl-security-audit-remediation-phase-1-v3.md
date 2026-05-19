# Implementation Review v3 — WCP Security Audit Remediation Phase 1

**Document:** `019-20260519T0603-impl-security-audit-remediation-phase-1-v3.md`\
**Design:** `019-20260516T0715-design-security-audit-remediation-v8.md` (WCP
v11)\
**Plan:** `019-20260518T0554-plan-security-audit-remediation-v3.md`\
**Prior reviews:**
`019-20260518T1745-impl-security-audit-remediation-phase-1-v1.md` (score 20),
`019-20260519T0500-impl-security-audit-remediation-phase-1-v2.md` (score 40)\
**Scope:** Phase 1, commits 1–6 + fixup autosquash set A–H\
**Reviewer:** Staff Engineer (independent, adversarial)

---

## Executive Summary

The fixup set E–H delivered by the implementer since v2 closes all five original
blockers/majors (F1–F5) and three of four v2 new findings (N1, N2, N4). N3 is
partially addressed: the extracted `handle_dispatch_send_failure` helper is
tested in isolation, but the try_dispatch call site at step 12 has no
integration test asserting that the helper is actually invoked on send failure.
The remaining open item is not a blocking correctness defect, but it represents
an unverifiable contract between the refactored site and its helper.

Two pre-existing dispatch.rs sites were not consolidated as part of fixup E
because `cleanup_terminal_state` is declared `async fn` in `handler.rs` with no
pub(crate) visibility, making it inaccessible from `dispatch.rs`. One of those
sites — the integrity-check rejection path in `handle_build_rejected` — omits
`set_build_log_finished`, which will cause SSE log streams for
integrity-rejected builds to hang indefinitely. This is a real correctness bug,
though pre-existing and not introduced by the Phase 1 fixup work.

**Score: 85/100. Conditional approve.** The Phase 1 scope is sound for merging.
The two dispatch.rs issues must be tracked as mandatory follow-on before Phase 2
ships, since Phase 2 adds more paths through `handle_build_rejected` and the
hang surface will grow.

---

## Finding Status

### Carried from v1 (F1–F5)

| ID  | v1 Status                                                  | v3 Status  | Evidence                                                                               |
| --- | ---------------------------------------------------------- | ---------- | -------------------------------------------------------------------------------------- |
| F1  | BLOCKER — `cleanup_terminal_state` absent                  | **Closed** | `handler.rs:753–777`; `fail_build` and "revoking" arm both delegate                    |
| F2  | BLOCKER — `.sqlx/` stale                                   | **Closed** | `.sqlx/query-49ca4f1…json` present; `SQLX_OFFLINE=true cargo check` passes             |
| F3  | BLOCKER — `WorkerStatus` missing from `WorkerBuildAction`  | **Closed** | `ws.rs:73`; `WorkerStatus` is first variant                                            |
| F4  | MAJOR — `authorize_lifecycle_message` not under queue lock | **Closed** | `dispatch.rs:561`; `.receipt = ReceivedByWorker` set inside `queue.active` write guard |
| F5  | MINOR — `idle_reconcile_decision` test not gating shutdown | **Closed** | Function exercised; state-machine coverage confirmed                                   |

### Carried from v2 (N1–N4)

| ID  | v2 Status                                                                  | v3 Status   | Evidence                                                                                                                                                    |
| --- | -------------------------------------------------------------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| N1  | NEW — three inline cleanup copies                                          | **Closed**  | `fail_build` at `handler.rs:1134–1144` and "revoking" arm at `handler.rs:1102–1110` both delegate to `cleanup_terminal_state`; no fourth copy in handler.rs |
| N2  | NEW — fixup F assertion untested / `.sqlx/` stale                          | **Closed**  | Offline cache JSON present; migration `001_initial_schema.sql:114` confirms `finished` column; assertion at `handler.rs:1535` is sound                      |
| N3  | NEW — `handle_dispatch_send_failure` helper never called from try_dispatch | **Partial** | See NA1 below                                                                                                                                               |
| N4  | NEW — `take_reconnect_messages` let-chain unsafe                           | **Closed**  | `edition = "2024"` confirmed; `Snapshot.build_id` is owned `BuildId`; no reference borrowing issues                                                         |

---

## New Findings (v3)

### NA1 — N3 Partially Open: try_dispatch call site unguarded (Minor, D5)

**Location:** `cbsd-server/src/ws/dispatch.rs`, step 12 refactor\
**Status:** N3 is closed for the helper itself. The new test
`handle_dispatch_send_failure_clears_six_columns_and_requeues`
(`dispatch.rs:992–1055`) exercises the helper directly and asserts all six WCP
D4 provenance columns are NULL, the ack is cancelled, the watcher is removed,
and the build is requeued. This is high-quality coverage.

What remains: the test does not reach step 12 of `try_dispatch`. If the
delegation line at `dispatch.rs:211–222` were accidentally reverted to inline
code, the test would still pass. The contract between step 12 and its helper is
asserted only by code inspection, not by an integration test that exercises the
full dispatch path and induces a send failure.

**Impact:** Low in isolation. The delegation is one line and is obviously
correct by inspection. The risk is regression during Phase 2 refactors when
`try_dispatch` is touched again.

**Resolution direction:** Add one integration test that causes the WebSocket
send at step 12 to fail (e.g., by closing the channel before calling
`try_dispatch`) and asserts the six columns are NULL afterward. This test can be
added to Phase 2 without blocking Phase 1 merge, provided it is tracked as a
Phase 2 entry requirement.

---

### NA2 — `handle_build_rejected` integrity path missing `set_build_log_finished` (Major, D2/D9)

**Location:** `cbsd-server/src/ws/dispatch.rs:488–506`\
**Root cause:** `cleanup_terminal_state` is declared `async fn` in `handler.rs`
with module-private visibility. When fixup E consolidated the two inline copies
inside `handler.rs`, the two inline copies inside `dispatch.rs` were not in
scope for refactoring. The implementer addressed one of them (the six-column
rollback in `handle_dispatch_send_failure`) via its own helper, but left the
integrity-check rejection path unchanged.

**Defect:** The integrity-check arm of `handle_build_rejected` performs three of
the four canonical terminal-cleanup steps:

1. `set_build_finished` — present
2. `set_build_log_finished` — **missing**
3. `queue.active.remove` — present
4. `log_watchers.remove` — present

The missing step 2 means that after an integrity rejection, the
`build_logs.finished` column remains `0` (false). The SSE log-streaming handler
polls on that column via the `watch::Sender` mechanism. With `finished` never
set, any client that opened an SSE stream for the rejected build will wait for
the full stream-timeout before receiving an EOF signal. On a busy system with
many integrity rejections, this accumulates open file descriptors and stalled
HTTP connections.

**Pre-existing:** This bug exists in the base commits before the Phase 1 fixup
work. Fixup E consolidated the handler.rs copies but could not reach
dispatch.rs. It is not introduced by Phase 1.

**Why it matters now:** Phase 2 adds more code paths through
`handle_build_rejected`. Until `cleanup_terminal_state` is promoted to
`pub(crate)` and imported by dispatch.rs, each new Phase 2 path is at risk of
repeating the omission.

**Resolution direction:**

1. Promote `cleanup_terminal_state` to `pub(crate)` in `handler.rs` (one-line
   visibility change).
2. Import and call it from `dispatch.rs:handle_build_rejected` integrity arm,
   replacing the three-step inline sequence.
3. Add `set_build_log_finished` to `handle_revoke_timeout`
   (`dispatch.rs:719–747`) in the same commit to close the second dispatch.rs
   copy.
4. Add a test asserting `build_logs.finished = 1` after an integrity rejection.

This is four lines of production change and one test. It should be commit 1 of
Phase 2, before any new rejection paths are added.

---

### NA3 — `handle_revoke_timeout` inline cleanup copy not refactored (Minor, D2)

**Location:** `cbsd-server/src/ws/dispatch.rs:719–747`\
**Status:** This is the fourth instance of the four-step cleanup sequence. It
correctly includes all four steps (unlike NA2), so it does not introduce a
correctness defect. However, it remains a structural duplication risk: any
future change to the canonical cleanup sequence (e.g., adding a fifth step) must
be applied to this copy manually.

**Impact:** Low correctness risk today. Grows with each Phase 2 addition that
touches the cleanup sequence.

**Resolution direction:** Resolved in the same commit as NA2 (step 3 above). No
separate action needed.

---

## Cross-Commit Invariant Verification

| Invariant                                                             | Status   | Evidence                                                                                                     |
| --------------------------------------------------------------------- | -------- | ------------------------------------------------------------------------------------------------------------ |
| WCP D4 SI-6/SI-13: 6 provenance columns cleared on rollback           | Pass     | `handle_dispatch_send_failure` test asserts all six NULL; production path at `dispatch.rs:357–383` confirmed |
| `PRAGMA foreign_keys = ON` per-connection                             | Pass     | `connect_options.pragma("foreign_keys", "ON")` in server init                                                |
| `set_build_log_finished` called on every terminal path in handler.rs  | Pass     | All handler.rs terminal paths go through `cleanup_terminal_state`                                            |
| `set_build_log_finished` called on every terminal path in dispatch.rs | **Fail** | `handle_build_rejected` integrity arm omits step 2 (NA2)                                                     |
| `cleanup_terminal_state` 4-step ordering preserved                    | Pass     | Steps ordered: DB write → DB write → memory remove → memory remove                                           |
| SQLite pool sizing ≤ 4 (deadlock guard)                               | Pass     | `max_connections = 4` in pool config                                                                         |
| `.sqlx/` offline cache covers all queries                             | Pass     | `SQLX_OFFLINE=true cargo check --workspace` passes clean                                                     |
| Golden rule: every post-squash commit compiles                        | Pass     | `SQLX_OFFLINE=true cargo check --workspace` — Finished in 1.92s, 0 errors                                    |

---

## Confidence Score

| Item                             | Points | Description                                                                                           |
| -------------------------------- | ------ | ----------------------------------------------------------------------------------------------------- |
| Starting score                   | 100    |                                                                                                       |
| N3 partial (D5)                  | -5     | try_dispatch step 12 call site has no integration test; helper tested in isolation only               |
| NA2: SSE hang bug (D9)           | -5     | `handle_build_rejected` integrity path missing `set_build_log_finished`; pre-existing but unaddressed |
| NA3: duplicate cleanup copy (D2) | -5     | `handle_revoke_timeout` inline copy not refactored; structural debt                                   |
| **Total**                        | **85** |                                                                                                       |

**Interpretation:** 85 — Acceptable with noted improvements. The Phase 1 scope
is correct and safe to merge. The two dispatch.rs issues must be resolved before
Phase 2 adds new rejection paths.

---

## Recommendation

**Conditional approve for Phase 1 merge.**

Required before Phase 2 begins (not before Phase 1 merge):

1. **(Mandatory)** Promote `cleanup_terminal_state` to `pub(crate)` and use it
   in `dispatch.rs:handle_build_rejected` and `handle_revoke_timeout`. Add test
   asserting `build_logs.finished = 1` after integrity rejection. This closes
   NA2 and NA3 together.
2. **(Recommended)** Add an integration test that induces a send failure through
   the full `try_dispatch` path and asserts the six provenance columns are NULL.
   Closes N3 completely.

Both items are scoped to a single small commit that can be Phase 2, commit 1
without disturbing Phase 2's planned scope.
