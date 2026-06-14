# Impl Review — Dead Worker Resolution (audit-rem D12) — v1

- Commit: `4cb930a8` — "cbsd-rs/server: resolve dead workers by DB state and
  receipt"
- Scope: server-only (`cbsd-server/src/ws/handler.rs`,
  `cbsd-server/src/queue/mod.rs`)
- Design: `docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md`
  (D12)
- Plan: `docs/cbsd-rs/plans/019-20260516T1033-security-audit-remediation.md`
  (Commit 20)
- Reviewer model: claude-opus-4-8
- Verdict: **GO — confidence 100/100**

## Scope under review

Single commit `4cb930a8`, server-only, two files:

- `cbsd-server/src/queue/mod.rs` (+12/-?): renamed
  `active_builds_for_connection` → `active_builds_with_receipt_for_connection`,
  now returning `Vec<(i64, ActiveAssignmentReceipt)>` instead of `Vec<i64>`.
- `cbsd-server/src/ws/handler.rs` (+295/-80 net across the file): added pure
  `dead_worker_resolution(db_state, receipt) -> DeadWorkerAction`; added
  per-build executor `resolve_dead_build(...)` taking explicit state pieces;
  rewrote `handle_worker_dead` to snapshot `(id, receipt)` and delegate; removed
  the now-unused `fail_build` wrapper; added 5 pure-fn unit tests + 4
  `#[tokio::test]` integration tests.

The implementer note that the plan oversold scope (calling out
`liveness.rs::handle_worker_dead`) is accurate: the resolver lives in
`handler.rs`; `liveness.rs` only runs the grace monitor that _calls_
`handle_worker_dead` and is untouched by this commit. Confirmed by the diff
touching only the two files above. The plan divergence is a documentation
artifact, not a code defect, and is not flagged as such (per review charter).

## Design fidelity — the 4-row table

Design D12 table (design/019, ~L1082–1097) vs `dead_worker_resolution`:

| DB state / receipt                | Design action / reason                              | Code action / reason                               | Match |
| --------------------------------- | --------------------------------------------------- | -------------------------------------------------- | ----- |
| `dispatched` / `AwaitingReceipt`  | Roll back to `queued`                               | `RollbackToQueued`                                 | ✓     |
| `dispatched` / `ReceivedByWorker` | `failure`, "worker died after accepting assignment" | `Fail("worker died after accepting assignment")`   | ✓     |
| `started` / (any)                 | `failure`, "worker died during execution"           | `Fail("worker died during execution")`             | ✓     |
| `revoking` / (any)                | `revoked` (revoke completed by worker death)        | `MarkRevoked("revoke completed by worker death")`  | ✓     |
| other / (any)                     | (unspecified — pre-existing catch-all)              | `RemoveOnly` (drop active entry, no DB transition) | ✓     |

All four design rows and the exact reason strings match byte-for-byte. The
reason-string alignment claimed in the commit message holds: `started`'s old
"worker lost" is now "worker died during execution" per the design.

The `dispatched` arm is correctly split by receipt — this is the single
behavioral bug the commit fixes. Before, `handle_worker_dead`'s `"dispatched"`
arm called `rollback_active_to_queued` unconditionally, ignoring the receipt, so
a `ReceivedByWorker` build (worker may have side-effected to Harbor/S3) was
re-queued and could double-execute. The fix routes `ReceivedByWorker` to `Fail`
(no enqueue) and keeps `AwaitingReceipt` on the rollback path. Design fidelity
confirmed.

## Adversarial probes

**Pure-fn correctness.** `dead_worker_resolution` matches all four design rows
plus the catch-all (see table above). Verified against design L1082–1097.

**KEY guarantee — `ReceivedByWorker` does NOT requeue.** Traced both paths in
the real code:

- `RollbackToQueued` → `dispatch::rollback_active_to_queued` (dispatch.rs:301)
  which calls `q.enqueue_front(QueuedBuild { ... })` at L324 — it DOES
  re-enqueue.
- `Fail` → `cleanup_terminal_state` (handler.rs:828) which calls
  `set_build_finished` + `set_build_log_finished`, then `q.active.remove` and
  `watchers.remove` — there is NO `enqueue`/`enqueue_front` anywhere in it.

So `dispatched + ReceivedByWorker` → `Fail` → terminal, no requeue. The
double-execution guard holds. This is asserted directly by
`resolve_dead_dispatched_received_fails_and_does_not_requeue`, which checks BOTH
`state == "failure"` AND `!q.contains(BuildId(build_id))` (see Tests).

**Divergence from `idle_reconcile_decision` is sound.** In
`idle_reconcile_decision` (handler.rs:736), `dispatched + ReceivedByWorker`
rolls back to queued; here it fails. The distinction is worker-back vs
worker-gone. Dead-worker resolution is the strictly _more conservative_ branch
of the same receipt split: when the worker's fate is unknown (it accepted the
assignment then vanished), fail rather than requeue, because a requeue could
double-execute work that may already have side-effected to Harbor/S3. The
idle-reconnect path makes its own (separately-reviewed) tradeoff for a worker
that is back and reporting idle; this commit does not depend on or vouch for
that tradeoff — it only has to be no less safe, and failing-without-requeue is.
The divergence is documented in `dead_worker_resolution`'s doc comment and is
sound for this commit's scope. A related, also-correct divergence: the catch-all
is `Skip` in `idle_reconcile_decision` (worker alive, leave the entry) but
`RemoveOnly` here (worker gone, drop the stale entry).

**Snapshot race / TOCTOU.** `handle_worker_dead` snapshots `(build_id, receipt)`
under the queue lock, then releases it before per-build resolution.
`ActiveAssignmentReceipt` is `#[derive(... Copy)]` (queue/mod.rs:31), so the
snapshot is a by-value copy — the receipt cannot change after snapshot. The
worker is already declared dead (no live connection), so no new owned message
can advance `AwaitingReceipt → ReceivedByWorker` in the window; the in-code
comment states exactly this. The DB `get_build` read inside `resolve_dead_build`
happens without the queue lock, but the action helpers
(`rollback_active_to_queued`, `cleanup_terminal_state`) each re-acquire the
queue lock and use `active.remove`, which is idempotent if the entry is already
gone (returns `None`, no panic). No lock is held across an `.await` in
`resolve_dead_build` (the `get_build` await completes before any lock is taken).
Acceptable: this is the same lock discipline the prior code used, and the
worker-gone invariant removes the only concurrent mutator. No new race
introduced.

**Wiring.** Each action maps to the correct helper:
`RollbackToQueued`→`rollback_active_to_queued`,
`Fail`→`cleanup_terminal_state(Failure)`,
`MarkRevoked`→`cleanup_terminal_state(Revoked)`, `RemoveOnly`→`q.active.remove`
under a freshly-acquired lock. Confirmed (not inferred from the comment) the
post-loop call `dispatch::try_dispatch(state)` at handler.rs:1167 — so builds
rolled back to `queued` are actually re-dispatched. The snapshot lock is
released at handler.rs:1150 before the resolution loop begins, so the queue lock
is not held across the per-build `.await`s. Wiring is correct.

## Regression hunt

**Removed `fail_build`.** `grep -rn fail_build` over `cbsd-server/src` and
`cbsd-worker/src` returns zero hits — fully removed, no dangling callers. The
only remaining callers of its body were `handle_worker_dead`'s `started` arm
(now routed through `Fail`→`cleanup_terminal_state`) and nothing else, so
removing the wrapper is clean. No dead code, no orphaned reference.

**Renamed helper.** `active_builds_for_connection` has zero references (old name
gone). `active_builds_with_receipt_for_connection` has exactly one caller
(handler.rs:1149) plus its definition (queue/mod.rs:220). Rename is complete.

**`started` reason change.** The dead-worker `started` reason moved from "worker
lost" to "worker died during execution". The only surviving "worker lost build"
string is in `idle_reconcile_one` (handler.rs:917) — a _different_ path,
untouched by this commit. No test asserts the old dead-worker `started` reason;
the only assertions on these strings are the new D12 tests (handler.rs:1862,
1882). The rename broke no test. (handler.rs:719 reads "the worker lost the
executing build" — but that is a doc comment on
`IdleReconcileAction::FailBuild`, the idle-reconnect path, whose reason string
("worker lost build") is unchanged by this commit. Not stale, not in scope.)

**`RemoveOnly` vs the old catch-all.** Both drop only the active map entry
(`queue.active.remove(&build_id)`); neither removes the log watcher. So
`RemoveOnly` is behavior-preserving relative to the prior catch-all — not a
regression introduced here. The watcher-not-removed concern is pre-existing and
out of scope for this commit (and the catch-all only fires for already-
terminal/queued states, where a dispatch-time watcher is normally already
cleaned up).

**`run_startup_recovery` restart rationale.** recovery.rs:36 selects
`WHERE state IN ('dispatched', 'started')` and updates all to `failure` with
`error = 'server restarted'` — the receipt is never consulted (it is in-memory
per WCP SI-25 and does not survive a restart). This confirms the commit's claim
that D12's receipt-aware table applies only within a live process; after
restart, all in-flight builds fail regardless of receipt. `run_startup_recovery`
has no dedicated unit test (confirmed: no test references it) — a pre-existing
gap, correctly scoped out by the implementer.

## Tests

`SQLX_OFFLINE=true cargo test -p cbsd-server` → **184 passed; 0 failed**
(matches the implementer's claim). The 9 new D12 tests are all present and
green:

Pure-fn rows (5):

- `dead_worker_dispatched_awaiting_rolls_back`
- `dead_worker_dispatched_received_fails` (asserts exact reason string)
- `dead_worker_started_fails_regardless_of_receipt` (loops both receipts)
- `dead_worker_revoking_marks_revoked_regardless_of_receipt` (loops both)
- `dead_worker_terminal_or_queued_removes_only`
  (`success`/`failure`/`revoked`/`queued`)

Integration rows (4, `#[tokio::test]` against a real SQLite pool):

- `resolve_dead_dispatched_awaiting_rolls_back_to_queued` — asserts
  `state == "queued"`, active entry removed, AND `q.contains(...)`
  (re-enqueued).
- `resolve_dead_dispatched_received_fails_and_does_not_requeue` — the KEY test.
  Asserts BOTH `state == "failure"` with the exact reason AND
  `!q.contains(BuildId(build_id))` ("must NOT requeue — double-exec guard").
  This is the direct empirical proof of the no-requeue guarantee.
- `resolve_dead_started_marks_failure` — `failure` + "worker died during
  execution".
- `resolve_dead_revoking_marks_revoked` — `revoked` + "revoke completed by
  worker death".

Coverage of the two critical paths (`dead_worker_resolution` pure fn and
`resolve_dead_build` executor) is complete. The only untested path is
`handle_worker_dead` itself (the snapshot loop + post-loop `try_dispatch`), but
its logic is trivial delegation to the tested `resolve_dead_build` plus an
existing dispatch call — no D5 deduction warranted, as the resolver it wraps is
fully covered and `try_dispatch` is independently tested. The server-restart row
has no new test by design (receipt is in-memory; `run_startup_recovery` covers
it and is a pre-existing untested gap, scoped out).

## Commit hygiene (git-commits smell test)

Single commit, ~295 net added lines across two files — within the 400–800 target
band for authored LOC (the bulk is tests + doc comments).

1. **One-sentence purpose** — yes: "make dead-worker resolution receipt-aware so
   an accepted-but-unfinished dispatch is failed, not requeued."
2. **Previous commit compiles** — the change is self-contained (rename + new
   fn + rewrite of one existing fn + tests); parent builds independently.
3. **Revertable** — reverting restores the prior unconditional rollback with no
   collateral; the rename reverts cleanly (single caller).
4. **Testable** — 9 new tests verify the new behavior that did not exist before.
5. **No dead code** — `dead_worker_resolution` and `resolve_dead_build` each
   have a caller (`resolve_dead_build` and `handle_worker_dead` respectively);
   the new `DeadWorkerAction` variants are all constructed and matched;
   `fail_build` was removed rather than left orphaned. Clippy `-D warnings`
   passes (no `#[allow(dead_code)]` added on the new items).

**Message quality** — component prefix `cbsd-rs/server` is correct and matches
the diff. Body explains the _why_ (double-execution risk from requeuing
`ReceivedByWorker`), the mechanism (pure table), and the restart rationale.
Exactly one `Co-authored-by` trailer and a `Signed-off-by`, per CLAUDE.md. No
stacked co-authors. `Closes audit-rem D12.` ties it to the design item.

No mixing of unrelated concerns: the queue-helper rename is a direct
prerequisite of receipt-aware resolution (the resolver needs the receipt), so it
belongs in this commit, not a separate one.

## Confidence score

| Item                           | Points  | Description                                                                         |
| ------------------------------ | ------- | ----------------------------------------------------------------------------------- |
| Starting score                 | 100     |                                                                                     |
| D1: deferred work              | 0       | All 4 D12 rows implemented; restart row correctly out of scope (in-memory).         |
| D2: duplicated code            | 0       | Mirrors `idle_reconcile_*` deliberately; no copy-paste, distinct semantics.         |
| D5: untested critical path     | 0       | Both critical paths (pure fn + executor) fully tested; 9 new tests pass.            |
| D6: dead code                  | 0       | `fail_build` removed; all new items have callers; clippy `-D warnings` clean.       |
| D7: security gap               | 0       | Closes a double-execution hazard; no new gap introduced.                            |
| D9: observability gap          | 0       | Each action logs (warn/error) with build_id, connection_id, reason.                 |
| D10: convention violation      | 0       | fmt clean, clippy clean, commit form per CLAUDE.md (prefix, sign-off, 1 co-author). |
| D12: commit boundary violation | 0       | Single coherent commit; passes all five smell-test points.                          |
| **Total**                      | **100** |                                                                                     |

Verified empirically: `SQLX_OFFLINE=true cargo test -p cbsd-server` → 184/0;
`cargo clippy -p cbsd-server --all-targets -- -D warnings` clean;
`cargo fmt -p cbsd-server -- --check` clean.

## Findings by severity

**Critical (90–100):** none.

**Important (80–89):** none.

**Observations (informational, no deduction):**

- `handle_worker_dead` itself has no direct test (the integration tests drive
  `resolve_dead_build`). Its body is trivial delegation plus the verified
  `try_dispatch(state)` call at handler.rs:1167; the wrapped resolver is fully
  covered. No deduction.
- `RemoveOnly` drops the active entry but not the log watcher — behavior is
  identical to the prior catch-all; pre-existing, out of scope, and only fires
  for already-terminal/queued states.
- `run_startup_recovery` has no dedicated unit test (pre-existing gap); the
  restart rationale relies on it, but it is correctly scoped out of this commit.
- Plan's "Packages / ~400" snapshot line and the plan's overstated `liveness.rs`
  scope are documentation snapshots, intentionally left per policy — not code
  defects.

## Verdict

**GO.** Confidence 100/100. The commit faithfully implements the D12 4-row
resolution table with exact reason strings, correctly splits the `dispatched`
arm by receipt to close the double-execution hazard, introduces no dead code or
dangling references, and is fully covered by the key no-requeue test plus eight
others. Tests, clippy (`-D warnings`), and fmt verified green. No blocking or
important findings. Ready to merge.
