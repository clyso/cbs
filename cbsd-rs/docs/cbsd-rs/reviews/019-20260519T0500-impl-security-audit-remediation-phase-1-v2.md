# Review: Security Audit Remediation — Phase 1 (v2)

**Document:** `019-20260519T0500-impl-security-audit-remediation-phase-1-v2.md`
**Type:** Implementation review (adversarial, source-validated) **Reviewer:**
Staff Engineer (automated adversarial agent) **Design:**
`019-20260426T1154-worker-control-plane-hardening.md` (v11) **Plan:**
`019-20260518T0554-plan-security-audit-remediation-v3.md` **Preceding review:**
`019-20260518T1745-impl-security-audit-remediation-phase-1-v1.md` **Commits in
scope:** Fixup A (`40e6de19`), B (`cdd6ce10`), C (`b771c8f8`), D (`091add9d`) —
four autosquash fixup commits addressing v1 findings F1–F5

---

## 1. Summary Assessment

The four fixup commits collectively close all five v1 findings. Every blocker is
now addressed at the code level: the send-failure rollback (F1), the missing
`WorkerStatus` wire variant (F2), and the `"revoking"` wildcard routing (F3) are
all correctly resolved in the source. The integration-test gap (F4) is closed
with three real-SQLite tests. The lock-across-I/O smell (F5) is structurally
fixed.

Three new findings were identified. The most significant is a D2 triplication:
`cleanup_terminal_state` (new, fixup C), `fail_build` (pre-existing), and an
inline four-step block in `handle_worker_dead`'s `"revoking"` arm all perform
the same terminal cleanup sequence; only the idle reconciler path calls the new
helper. This divergence will compound as more terminal state paths are added.
Two additional minor gaps exist: the send-failure path lacks an end-to-end test
driving `try_dispatch` itself, and one integration test does not assert the
`build_logs.finished` column that its helper explicitly sets.

The implementation is **conditionally ready to proceed**. No finding blocks
correctness in production today, but the triplication (N1) should be resolved
before Phase 2 extends the number of terminal transitions.

---

## 2. v1 Finding Closure Status

| ID  | Title                                          | Status     | Notes                                                                                 |
| --- | ---------------------------------------------- | ---------- | ------------------------------------------------------------------------------------- |
| F1  | Send-failure skips DB rollback                 | **Closed** | Step 12 now calls `rollback_active_to_queued`; 4 NULL assertions added to helper test |
| F2  | `WorkerStatus` absent from `WorkerBuildAction` | **Closed** | Variant added as first arm; serde round-trip test added                               |
| F3  | `"revoking"` wildcarded to `Skip`              | **Closed** | `MarkRevoked` arm added before wildcard; positive + negative tests present            |
| F4  | No integration tests for idle reconcile        | **Closed** | `idle_reconcile_one` extracted; 3 real-SQLite integration tests added                 |
| F5  | Queue lock held across `drain_spool` I/O       | **Closed** | Restructured into 3 phases; lock released before all filesystem I/O                   |

### Verification details

**F1** — `dispatch.rs` step 12 (lines 205–226) calls
`rollback_active_to_queued`. The helper is defined in the same file (line 325,
added in commit 1 — pre-squash golden rule satisfied). The test
`rollback_active_to_queued_resets_db_and_reenqueues_at_front` now asserts
`error`, `started_at`, `finished_at`, and `build_report` are all NULL after
rollback. The fix is correct. See also N3 below.

**F2** — `cbsd-proto/src/ws.rs` `WorkerBuildAction` enum now reads
`WorkerStatus, BuildAccepted, BuildStarted, BuildOutput, BuildFinished, BuildRejected`.
The serde test `worker_build_action_worker_status_serdes_as_snake_case` verifies
the variant round-trips as `"worker_status"`. The callsite in `handler.rs:922`
was updated from `BuildStarted` to `WorkerStatus`. Fix is complete.

**F3** — `idle_reconcile_decision` now has an explicit arm
`"revoking" => IdleReconcileAction::MarkRevoked` before the wildcard. The test
`idle_other_states_are_skipped` no longer includes `"revoking"` in its set;
`idle_revoking_marks_revoked` tests all four `receipt × prev_live` combinations.
Fix is complete.

**F4** — `idle_reconcile_one` is extracted as a testable free function taking
pool, queue, log_watchers, candidate, and registered_worker_id. Three
integration tests were added:
`idle_reconcile_one_dispatched_awaiting_dead_prev_rolls_back_and_clears_db`,
`idle_reconcile_one_revoking_marks_revoked`, and
`idle_reconcile_one_skips_when_db_worker_id_mismatch`. All three use a real
in-memory SQLite pool. See also N2 below.

**F5** — `take_reconnect_messages` is restructured into three phases: snapshot
under lock (phase 1), filesystem I/O with no lock held (phase 2), brief
re-acquisition to zero `spool_bytes`/`spool_path` (phase 3). The lock is not
held across any `await` that touches the filesystem. Fix is correct; see N4 for
a theoretical nit.

---

## 3. New Findings

### N1 — D2 × 2: Terminal cleanup triplicated (Major)

**What:** Three independent copies of the four-step terminal cleanup sequence
exist in `handler.rs`:

1. `cleanup_terminal_state` (lines 753–777, added by fixup C) — called by the
   idle reconciler's `FailBuild` and `MarkRevoked` arms.
2. `fail_build` (lines 1147–1165, pre-existing) — called by the worker auth
   rejection path, the `"started"` arm of `handle_worker_dead`, and several
   other sites. Accepts an `&AppState`.
3. Inline block (lines 1096–1126) inside `handle_worker_dead`'s `"revoking"` arm
   — introduced during the WCP commit series and never refactored to use either
   helper.

All three execute the same logical sequence: `set_build_finished` →
`set_build_log_finished` → `queue.active.remove` → `log_watchers.remove`. The
only structural difference is parameter passing (`AppState` vs. individual
fields).

**Why it matters:** Adding a Phase 2 terminal transition means picking one of
three templates or writing a fourth copy. An omission in any one copy (e.g.,
forgetting `set_build_log_finished`) will silently produce inconsistent DB
state. The triplication was introduced by fixup C: the new helper was not used
to replace the `handle_worker_dead` inline block or to eliminate `fail_build`.

**Resolution:** Refactor `fail_build` and the `handle_worker_dead` inline block
to call `cleanup_terminal_state` (or a unified version that accepts `AppState`).
`cleanup_terminal_state` should become the single authoritative terminal cleanup
function. One approach: overload via a thin wrapper that accepts `&AppState` and
delegates to the field form.

**Deduction:** D2 × 2 (two duplicated function bodies) = −30

---

### N2 — D5: `idle_reconcile_one_revoking_marks_revoked` omits `build_logs.finished` assertion (Minor)

**What:** The integration test `idle_reconcile_one_revoking_marks_revoked`
asserts `build.finished_at.is_some()`, `build.status == "revoked"`, the active
map is cleared, and the watcher map is cleared. It does not query the
`build_logs` table to assert `finished = 1`.

`cleanup_terminal_state` explicitly calls `set_build_log_finished`, but no test
verifies this path for the `MarkRevoked` branch. If `set_build_log_finished`
were accidentally removed from `cleanup_terminal_state` or conditionalised, the
`MarkRevoked` test would continue to pass while SSE log streams for revoked
builds would hang indefinitely.

**Resolution:** Add a query `SELECT finished FROM build_logs WHERE build_id = ?`
and assert `finished == 1` inside the revoking integration test.

**Deduction:** D5 × 1 = −15

---

### N3 — D5: No end-to-end test for `try_dispatch` send-failure path (Minor)

**What:** F1's fix is correct: the send-failure branch at step 12 now calls
`rollback_active_to_queued`. The existing test exercises the helper in
isolation. But there is no test that constructs a broken sender, calls
`try_dispatch`, and asserts that the build is returned to the front of the queue
with all six provenance columns cleared.

If step 12 were accidentally reverted to the manual
`queue.active.remove + enqueue_front` block (which lacks the six-column DB
clear), the helper test would still pass and F1 would silently reopen.

**Resolution:** Add an integration test for `try_dispatch` that injects a
closed/disconnected sender so the send fails, then queries the DB to confirm the
build is `QUEUED` with `worker_id`, `trace_id`, `started_at`, `build_report`,
`error`, and `finished_at` all NULL, and that `queue.pending` contains the build
at its front.

**Deduction:** D5 × 1 = −15

---

### N4 — Theoretical build_id guard absent in supervisor phase 3 (Nit)

**What:** `take_reconnect_messages` phase 3 (lines 398–402) re-acquires the
supervisor lock and unconditionally zeroes `spool_bytes` and `spool_path` on the
current `active` build — whichever build is active at that moment. The comment
reads: "if another caller has retired the active build in the meantime, the
update is a no-op."

The comment correctly explains the `retire` case (`active = None`). It does not
address the case where `retire(old)` is followed by `register_accepted(new)`
between phase 1 and phase 3, which would overwrite the new build's spool state.

In practice this race cannot occur: `take_reconnect_messages` runs before the
message loop, and `register_accepted` can only fire from inside the message loop
after a `BuildNew` server message — these two contexts are sequential. The race
is not reachable in the current call graph.

**Resolution:** For clarity and long-term safety, guard phase 3 with a build-ID
check: `if active.build_id == snapshot.build_id`. This makes the invariant
explicit and costs one comparison.

**Deduction:** None (theoretical nit, not a production risk). Noted as a
code-quality improvement.

---

## 4. Cross-Commit Invariant Violations

**Golden rule (post-squash ordering):** Each post-squash commit must compile and
pass tests in isolation.

- Fixup A squashes into commit 1. Commit 1 defines `rollback_active_to_queued`;
  the fixup calls it from step 12. ✓
- Fixup B squashes into commit 2. `WorkerBuildAction` is defined in
  `cbsd-proto`; the new variant is self-contained. ✓
- Fixup C squashes into commit 3. `cleanup_terminal_state` and
  `idle_reconcile_one` are both defined and consumed within commit 3. ✓
- Fixup D squashes into commit 6. `take_reconnect_messages` restructuring is
  contained within the worker crate. ✓

No golden rule violations detected across the four fixup commits.

**WCP D4 invariant (6-column provenance clear on rollback):** Fixup A's
`rollback_active_to_queued` call delegates to the dedicated helper defined in
commit 1, which clears all six columns (`worker_id`, `trace_id`, `started_at`,
`finished_at`, `build_report`, `error`). The invariant holds for the dispatch
send-failure path. ✓

**sqlx offline cache:** Fixup C adds a new test-only query
(`UPDATE builds SET state = ?, worker_id = ?, trace_id = 't' WHERE id = ?`) and
commits the corresponding `.sqlx/query-b0aad2fe...json` file. No missing cache
entry detected. ✓

---

## 5. Confidence Score

| Item                                                                                       | Points | Description                                                 |
| ------------------------------------------------------------------------------------------ | ------ | ----------------------------------------------------------- |
| Starting score                                                                             | 100    |                                                             |
| N1: D2 — `cleanup_terminal_state` duplicates `fail_build` body                             | −15    | Same 4-step sequence in two named functions                 |
| N1: D2 — `handle_worker_dead` "revoking" inline duplicates `fail_build` body               | −15    | Same 4-step sequence as inline block                        |
| N2: D5 — `idle_reconcile_one_revoking_marks_revoked` omits `build_logs.finished` assertion | −15    | Critical path `set_build_log_finished` not verified in test |
| N3: D5 — No end-to-end test for `try_dispatch` send-failure path                           | −15    | F1 fix is not regression-guarded at the call site           |
| **Total**                                                                                  | **40** |                                                             |

---

## 6. Recommendation

**No-go on merging as-is.** The score of 40 places this work in the "major
rework needed" band under the confidence scoring rubric (0–49).

The primary driver is **N1**: introducing `cleanup_terminal_state` to fix F4
without consolidating the two pre-existing identical patterns added two D2
deductions that each carry the same weight as a security gap finding. Combined
with two untested critical paths (N2, N3), the total deduction is 60 points.

**Required before merge:**

1. Refactor `fail_build` and the `handle_worker_dead` "revoking" inline block to
   call `cleanup_terminal_state` (or a unified replacement). This eliminates N1
   and recovers 30 points.

2. Add `build_logs.finished = 1` assertion to
   `idle_reconcile_one_revoking_marks_revoked`. This eliminates N2 and recovers
   15 points. Projected score after 1+2: **85** (Acceptable).

3. Add a `try_dispatch` send-failure end-to-end test. This eliminates N3 and
   recovers an additional 15 points. Projected score after 1+2+3: **100**.

Item 3 is the hardest to implement (requires a controllable broken sender) but
is the most valuable regression guard for F1. Items 1 and 2 are straightforward
mechanical changes.

N4 (phase 3 build_id guard) may be addressed at the author's discretion; it
carries no scoring deduction and poses no production risk in the current call
graph.
