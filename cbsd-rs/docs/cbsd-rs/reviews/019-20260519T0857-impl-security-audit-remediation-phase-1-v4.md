# Implementation Review v4 — WCP Security Audit Remediation Phase 1

**Document:** `019-20260519T0857-impl-security-audit-remediation-phase-1-v4.md`\
**Design:** `019-20260516T0715-design-security-audit-remediation-v8.md` (WCP
v11)\
**Plan:** `019-20260518T0554-plan-security-audit-remediation-v3.md`\
**Prior reviews:**
`019-20260518T1745-impl-security-audit-remediation-phase-1-v1.md` (score 20),
`019-20260519T0500-impl-security-audit-remediation-phase-1-v2.md` (score 40),
`019-20260519T0603-impl-security-audit-remediation-phase-1-v3.md` (score 85)\
**Scope:** Phase 1, commits 1–6 + fixup autosquash set A–J\
**Reviewer:** Staff Engineer (independent, adversarial)

---

## Executive Summary

Fixups I (`e71405ce`) and J (`b5ffe946`) close all three v3 open items: NA2 and
NA3 are fully resolved by promoting `cleanup_terminal_state` to `pub(crate)` and
eliminating both dispatch.rs inline cleanup copies; NA1 is substantively
resolved by the `send_and_recover` helper extraction, though the
regression-guard shift noted below recategorises it from "partially open" to
"minor structural concern." Two nit-level findings are carried from the test
asymmetry and the compile-time-unenforced status string; one pre-existing
structural note on the `main.rs` drain path is recorded for completeness.

**Score: 92/100. Approve.** Phase 1 is ready to merge. The remaining findings
are non-blocking nits that do not represent correctness defects.

---

## Finding Status

### Carried from v1 (F1–F5)

| ID  | v1 Status                                                  | v4 Status  |
| --- | ---------------------------------------------------------- | ---------- |
| F1  | BLOCKER — `cleanup_terminal_state` absent                  | **Closed** |
| F2  | BLOCKER — `.sqlx/` stale                                   | **Closed** |
| F3  | BLOCKER — `WorkerStatus` missing from `WorkerBuildAction`  | **Closed** |
| F4  | MAJOR — `authorize_lifecycle_message` not under queue lock | **Closed** |
| F5  | MINOR — `idle_reconcile_decision` test not gating shutdown | **Closed** |

### Carried from v2 (N1–N4)

| ID  | v2 Status                                                             | v4 Status  |
| --- | --------------------------------------------------------------------- | ---------- |
| N1  | NEW — three inline cleanup copies                                     | **Closed** |
| N2  | NEW — fixup F assertion untested / `.sqlx/` stale                     | **Closed** |
| N3  | NEW — `handle_dispatch_send_failure` never called from `try_dispatch` | **Closed** |
| N4  | NEW — `take_reconnect_messages` let-chain unsafe                      | **Closed** |

### Carried from v3 (NA1–NA3)

| ID  | v3 Status                                                                       | v4 Status  | Evidence                                                                                                             |
| --- | ------------------------------------------------------------------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------- |
| NA1 | MINOR — `try_dispatch` step-12 call site unguarded by integration test          | **Closed** | `send_and_recover` extraction tested in isolation; regression-guard shift carried as NB1 (nit)                       |
| NA2 | MAJOR — `handle_build_rejected` integrity path missing `set_build_log_finished` | **Closed** | Fixup J: `crate::ws::handler::cleanup_terminal_state(...)` at `dispatch.rs:503–523`; all 4 steps confirmed present   |
| NA3 | MINOR — `handle_revoke_timeout` inline cleanup copy not refactored              | **Closed** | Fixup J: `crate::ws::handler::cleanup_terminal_state(...)` at `dispatch.rs:735–743`; behavior-equivalent replacement |

---

## New Findings (v4)

### NB1 — `send_and_recover` regression guard shifted (Nit, D5)

**Location:** `cbsd-server/src/ws/dispatch.rs`\
**Status:** NA1 closes as intended. The `send_and_recover` extraction is correct
and the helper is tested in isolation by two new tests
(`send_and_recover_with_closed_receiver_rolls_back_db` and
`send_and_recover_with_no_sender_for_connection_rolls_back_db`). Both tests call
`send_and_recover` directly without going through `try_dispatch`.

If a maintainer replaces the `send_and_recover(...).await?` call at
`dispatch.rs:191–203` with inline code that omits the rollback, both tests
continue to pass. The delegation contract is asserted by code inspection only.

**Impact:** The pattern is structurally sound at this level of the call stack
and is much simpler to inspect than the original multi-step inline block. The
risk is regression during Phase 2 refactors that touch `try_dispatch`. This is
the same category of concern as the original NA1 but at a lower abstraction
level. No deduction applied because the helper-level tests are high-quality and
the delegation is a single line.

**Resolution direction:** Add one integration test that exercises the full
`try_dispatch` path with a send failure and asserts the six provenance columns
are NULL. Can be Phase 2, commit 1 without blocking Phase 1 merge.

---

### NB2 — Second `send_and_recover` test missing active/watcher assertions (Nit, D5)

**Location:** `cbsd-server/src/ws/dispatch.rs`,
`send_and_recover_with_no_sender_for_connection_rolls_back_db`\
**Finding:** The first test
(`send_and_recover_with_closed_receiver_rolls_back_db`) asserts four
postconditions: DB rollback, active entry removed, log watcher removed, and ack
cancelled. The second test asserts only two: DB rollback and ack cancelled. The
active entry removal and log watcher removal assertions are absent.

**Impact:** The second test exercises a different failure mode (no sender for
the connection ID rather than a closed channel). The omitted assertions mean
that if `handle_dispatch_send_failure` were modified to skip active/watcher
removal in the no-sender case, the second test would not catch it. Correctness
is preserved by the first test for the common case; the gap is a coverage
asymmetry, not a production defect.

**Deduction:** -5 (D5, distinct untested path within the same critical helper).

**Resolution direction:** Add `assert_active_removed` and
`assert_watcher_removed` calls to
`send_and_recover_with_no_sender_for_connection_rolls_back_db`, matching the
postcondition set of the first test. Three lines.

---

### NB3 — `main.rs` drain path has inline 2-step cleanup not using `cleanup_terminal_state` (Pre-existing Structural Note)

**Location:** `cbsd-server/src/main.rs`, shutdown drain loop\
**Status:** Pre-existing. Not introduced by any Phase 1 commit.

The shutdown drain loop performs `set_build_finished` and
`set_build_log_finished` inline without delegating to `cleanup_terminal_state`.
It intentionally omits `queue.active.remove` and `log_watchers.remove` (the
queue is being torn down; removing from a map that is about to be dropped is
unnecessary). This is structurally correct for the shutdown path.

However, if `cleanup_terminal_state` gains a fifth step in Phase 2 (for example,
an audit-log write), the drain path will not automatically inherit it.

**Impact:** No current defect. Structural drift risk if the cleanup sequence
evolves. No deduction applied for Phase 1 scope.

**Resolution direction:** Document the drain path as an intentional partial
caller in the `cleanup_terminal_state` docstring. Consider whether Phase 2 can
add an overload parameter (`skip_queue_remove: bool`) to allow the drain path to
delegate safely.

---

### NB4 — `status: &str` in `cleanup_terminal_state` not compile-time enforced (Nit, D11)

**Location:** `cbsd-server/src/ws/handler.rs`, `cleanup_terminal_state`
signature\
**Finding:** The function accepts `status: &str` with valid values documented as
`"failure"` or `"revoked"`. A caller could pass any string and the compiler will
not object. Two values are used in practice: `"failure"` (handler.rs and
`handle_build_rejected`) and `"revoked"` (handler.rs `MarkRevoked` arm and
`handle_revoke_timeout`).

**Impact:** Not a defect in Phase 1. The set of callers is small and all pass
correct values. The risk is a future caller passing a misspelled status that is
accepted at compile time and writes an unrecognised value to
`build_logs.status`.

**Deduction:** -3 (D11, undocumented constraint on a shared utility function;
partial deduction because valid values are noted in the docstring but not
enforced).

**Resolution direction:** Introduce a `TerminalStatus` enum with `Failure` and
`Revoked` variants. Replace `status: &str` with `status: TerminalStatus`.
Implement `Display` to produce the string representation. This is a one-commit
refactor that can land in Phase 2 alongside the helper.

---

## Cross-Commit Invariant Verification

| Invariant                                                               | Status | Evidence                                                                                                                |
| ----------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------------------------------------- |
| WCP D4 SI-6/SI-13: 6 provenance columns cleared on rollback             | Pass   | `handle_dispatch_send_failure` test asserts all six NULL; confirmed at `dispatch.rs:357–383`                            |
| `PRAGMA foreign_keys = ON` per-connection                               | Pass   | `connect_options.pragma("foreign_keys", "ON")` in server init                                                           |
| `set_build_log_finished` called on every terminal path in `handler.rs`  | Pass   | All 5 handler.rs terminal paths route through `cleanup_terminal_state`                                                  |
| `set_build_log_finished` called on every terminal path in `dispatch.rs` | Pass   | `handle_build_rejected` integrity arm now calls `cleanup_terminal_state` (NA2 closed); `handle_revoke_timeout` likewise |
| `cleanup_terminal_state` 4-step ordering preserved                      | Pass   | Steps ordered: DB write → DB write → memory remove → memory remove                                                      |
| `cleanup_terminal_state` visibility sufficient for all callers          | Pass   | `pub(crate)` in handler.rs; all dispatch.rs call sites use `crate::ws::handler::cleanup_terminal_state`                 |
| SQLite pool sizing ≤ 4 (deadlock guard)                                 | Pass   | `max_connections = 4` in pool config                                                                                    |
| `.sqlx/` offline cache covers all queries                               | Pass   | `SQLX_OFFLINE=true cargo check --workspace` — 1.88s, 0 errors, 0 warnings                                               |
| Golden rule: every post-squash commit compiles                          | Pass   | `SQLX_OFFLINE=true cargo check --workspace` passes clean across full commit sequence                                    |
| No orphan code introduced by fixups I or J                              | Pass   | Both `send_and_recover` and updated `cleanup_terminal_state` have callers in the same commit                            |

---

## Confidence Score

| Item                                           | Points | Description                                                                                |
| ---------------------------------------------- | ------ | ------------------------------------------------------------------------------------------ |
| Starting score                                 | 100    |                                                                                            |
| NB2: second `send_and_recover` test incomplete | -5     | Active/watcher removal not asserted in the no-sender test case; asymmetric with first test |
| NB4: `status: &str` undocumented constraint    | -3     | Valid values documented but not compile-time enforced; future caller risk                  |
| **Total**                                      | **92** |                                                                                            |

**Interpretation:** 92 — Ready to merge. Remaining findings are non-blocking
nits. Phase 2 entry requirements below.

---

## Recommendation

**Approve for Phase 1 merge.**

Required before Phase 2 begins (not before Phase 1 merge):

1. **(Recommended)** Add `assert_active_removed` and `assert_watcher_removed` to
   `send_and_recover_with_no_sender_for_connection_rolls_back_db`. Closes NB2.
   Three lines.
2. **(Recommended)** Add one integration test through the full `try_dispatch`
   path with a send failure, asserting six provenance columns are NULL. Closes
   NB1 regression-guard concern completely.
3. **(Optional)** Introduce `TerminalStatus` enum to replace `status: &str` in
   `cleanup_terminal_state`. Closes NB4 and prevents future misuse.

Items 1 and 2 are suitable as a single small commit at Phase 2, commit 1. Item 3
can be deferred to any Phase 2 slot without risk.
