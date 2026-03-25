# Design & Plan Review: Worker Registration v3 (Phase 7)

**Documents reviewed:**


- `cbsd-rs/docs/cbsd-rs/design/004-20260316T0925-worker-registration.md` (revised)
- `cbsd-rs/docs/cbsd-rs/plans/004-20260316T1018-01-worker-registration.md` (revised)

---

## Summary

All prior critical issues are resolved. The force-disconnect deadlock fix
is correctly narrated (lock queue → extract → remove → release → remove
from `worker_senders` → call `handle_worker_dead`). The reconnection race
is fixed by atomic connection migration under a single queue lock. The
`worker_senders` cleanup is an explicit named step. Priority preservation
on re-queue, `Stopping` in the merged listing, and `SeedWorker.arch: Arch`
are all addressed.

No blockers. Two significant concerns remain, both in the Commit 3
reconnection path. One is a lock-ordering ambiguity that could cause a
lock inversion if implemented naively. The other is an incomplete
specification for the double-connect `Connected` case. Both are small
targeted additions to existing narration.

**Verdict: Approve with conditions.** Fix the two significant concerns
before implementing Commit 3. Everything else can be addressed during
implementation.

---

## Prior Issues — All Resolved

| Prior issue | Status |
|---|---|
| C1 (v2): Force-disconnect deadlock | ✓ Explicitly releases queue lock before `handle_worker_dead` |
| C2 (v2): Reconnection race (grace-period monitor fails build) | ✓ Atomic migration under single queue lock |
| C3 (v2): Missing `worker_senders` cleanup | ✓ Named step: remove from `worker_senders` after queue lock released |
| M1 (v2): `handle_worker_dead` priority hardcoded `Normal` | ✓ Fixed to use `ab.priority` in Commit 3 |
| M2 (v2): `Stopping` missing from Commit 6 merge | ✓ Explicitly maps `Stopping → "stopping"` |
| M3 (v2): Arch validation duplicated | ✓ `SeedWorker.arch: Arch` — serde validates at parse time |
| B1–B7 (v1): All 7 original blockers | ✓ All verified resolved in v2 |

---

## Critical Issues

None.

---

## Significant Concerns

### S1 — Lock inversion risk in reconnection migration path

The plan's Commit 3 reconnection migration says:

> remove old entry, remove old `connection_id` from `worker_senders`,
> register the new connection. **All under one lock.**

`worker_senders` is a separate `Arc<Mutex<...>>` from the queue mutex.
The existing `cleanup_worker` acquires `worker_senders` first, then
`queue`. If the reconnect migration nests `worker_senders.lock()` inside
the queue lock, it creates a lock inversion. A deadlock would materialize
if `cleanup_worker` for the old connection runs concurrently.

**Fix:** Clarify that "all under one lock" refers only to the queue map
operations (migrate active build connection_id, remove old entry, register
new entry). The `worker_senders` removal happens **after** the queue lock
is released. Add one sentence: "After releasing the queue lock, remove the
old `connection_id` from `worker_senders`. This is safe because the grace-
period monitor needs the queue lock — it cannot fire during the migration
window."

### S2 — Double-connect `Connected` case leaves stale sender and orphaned build

The plan handles the double-connect case with:

> If found and `Connected` (stale double-connect): remove old entry,
> log warning, register new.


Not specified:

1. The old connection's `worker_senders` entry — the forward task keeps
   running, the old socket stays open.
2. Any active build in `queue.active` assigned to the old connection —
   it hangs permanently (the new connection doesn't know about it).

**Fix:** Treat the double-connect case identically to force-disconnect:
(1) remove old entry from queue map, (2) after releasing queue lock,
remove old `connection_id` from `worker_senders`, (3) call
`handle_worker_dead(state, old_connection_id)` to re-queue any active
build. Same sequence as deregistration — same lock-release reasoning.

---

## Minor Issues

- **Ack-cancel token during reconnect within ack window.** A worker that
  reconnects between `BuildNew` dispatch and its `BuildAccepted` response
  could have its build re-queued by the ack timeout task after connection
  migration completes. The ack timeout task holds a `CancellationToken`
  clone keyed to the old `ActiveBuild` entry, which was removed and
  replaced during migration. If the new `ActiveBuild` gets a fresh token,
  the old timeout fires and finds no matching entry — harmless no-op.
  If the migration preserves the old entry (just updating connection_id),
  the old token is still valid and `build_accepted` on the new connection
  cancels it. Worth a code comment acknowledging the race.

- **`requeue_active_build` also has the `Priority::Normal` hardcode.**
  The plan fixes `handle_worker_dead`, but `requeue_active_build` (if it
  exists as a separate function) should also use `ab.priority`.

- **TOCTOU on name uniqueness (step 4, `register_worker`).** The
  pre-transaction check races with concurrent registration. The UNIQUE
  constraint is the real guard. Map the sqlx `UniqueViolation` error to
  409 `"worker name already exists"` in the transaction error handler.

- **`WorkerRow.arch: String` → `Arch` parse in Commit 3.** The arch
  mismatch validation compares `Hello.arch` (enum) against
  `worker_row.arch` (String). Use `worker_row.arch.parse::<Arch>()`
  with a clear error, not implicit string comparison.

- **201 response contains plaintext API key.** Ensure no response-body
  logging middleware captures it. Add a handler comment.

---

## Suggestions

- **Log when both `CBSD_WORKER_TOKEN` env var and config file
  `worker_token` are set.** State which wins. Common source of confusion
  in containerized deployments.

- **`WorkerToken.arch` as `Arch` instead of `String`.** Token arch values
  can only be those the server accepted at registration — `Arch` is
  strictly safer and matches the rest of the proto crate.

- **Zero-downtime key rotation is intentionally not supported.** One
  sentence in the design noting this as a known limitation prevents future
  questions about dual-key verification grace windows.

---

## Strengths

- **All 10+ issues across 3 review passes resolved** with correct,
  concrete fixes. No regressions.
- **Force-disconnect sequence is precisely correct.** Lock ordering,
  `worker_senders` cleanup, and `handle_worker_dead` call are all spelled
  out with the non-reentrancy rationale.
- **Reconnection migration under single lock** prevents the grace-period
  monitor race. Correctly reasons about the lock as the synchronization
  boundary.
- **Crash-safe token rotation.** Insert new → update FK → revoke old
  ordering means any crash leaves at least one valid key.
- **Argon2 consistently outside transactions** in all three sites.
- **`SeedWorker.arch: Arch`** as the single validation source — serde
  validates at parse time, SQL CHECK is the DB safety net.
- **Priority preservation on re-queue** fixed in the right commit (3)
  alongside the reconnection and force-disconnect paths.
- **Commit dependency graph is accurate** and the deferred force-disconnect
  approach is safely documented.
