# Design & Plan Review: Worker Registration v2 (Phase 7)

**Documents reviewed:**


- `cbsd-rs/docs/cbsd-rs/design/004-20260316T0925-worker-registration.md` (revised)
- `cbsd-rs/docs/cbsd-rs/plans/004-20260316T1018-01-worker-registration.md` (revised)

**Cross-referenced against:** existing implementation source files

---

## Summary

This revision cleanly resolves all 7 blockers and 4 major concerns from
the prior review. The transaction ordering is correct (argon2 before tx,
insert new → update FK → revoke old), `workers:manage` is in `KNOWN_CAPS`,
`revoke_api_key_by_id` bypasses the owner filter, `ON DELETE CASCADE` is
in the DDL, `last_seen` updates on `build_finished`, the upgrade path is
documented as fresh-DB-only, and the Commit 2/3 dependency is resolved by
deferring force-disconnect to Commit 3.

Three new issues found, all in the Commit 3 force-disconnect and
reconnection paths. The most critical is a deadlock: the plan narrates
holding the queue mutex across `handle_worker_dead`, which re-acquires
the same non-reentrant mutex. The second is a reconnection race where the
grace-period monitor can fail an in-flight build before the reconnecting
worker sends `WorkerStatus`. The third is a missing `worker_senders`
cleanup step in the force-disconnect path.

**Verdict: Approve with conditions.** Fix C1–C3 before implementing
Commit 3. The remaining concerns can be addressed during implementation.

---

## Blockers (from prior review)

All 7 resolved:

| Prior blocker | Resolution | Verified |
|---|---|---|
| B1: Registration not transactional | Argon2 before tx, `insert_api_key_in_tx` + `last_insert_rowid()` | ✓ |
| B2: `workers:manage` missing from `KNOWN_CAPS` | Added in Commit 2 | ✓ |
| B3: `deregister_worker` wrong revocation call | New `revoke_api_key_by_id(pool, api_key_id)` | ✓ |
| B4: `regenerate-token` not atomic | Insert new → update FK → revoke old → commit | ✓ |
| B5: `WorkerState` Commit 2/3 dependency | Force-disconnect deferred to Commit 3 | ✓ |
| B6: Argon2 `spawn_blocking` inside tx | Hash before opening tx in all 3 sites | ✓ |
| B7: Legacy mode breaks upgrade path | Fresh DB required, documented explicitly | ✓ |

---

## Critical Issues (new)

### C1 — Force-disconnect deadlocks: queue mutex held across `handle_worker_dead`

The Commit 3 force-disconnect sequence narrates:

> 1. Lock `BuildQueue.workers`, scan for connection.
> 2. Remove entry from map.
> 3. Drop WS sender channel.
> 4. Call `handle_worker_dead(state, connection_id)`.

`handle_worker_dead` immediately calls `state.queue.lock().await` — a
second acquisition of the same non-reentrant `tokio::sync::Mutex`. This
deadlocks the task, holding the queue mutex indefinitely. All subsequent
queue operations block until the server is restarted.

**Fix:** Release the queue mutex before calling `handle_worker_dead`. The
correct sequence:

1. Lock queue → scan by `registered_worker_id` → extract `connection_id`
   → remove entry from map.
2. **Release queue lock.**
3. Remove `connection_id` from `state.worker_senders` (drops sender,
   triggers socket close).
4. Call `handle_worker_dead(state, connection_id)`.

Add one sentence to the design and plan: "The queue mutex must be released
before calling `handle_worker_dead`."

### C2 — Reconnecting worker loses in-flight build due to missing connection migration

The design's reconnection flow specifies:
1
> 1. Server checks `BuildQueue.workers` for any existing entry with the
> 2  same worker UUID (from a previous connection that is now `Disconnected`).
> 2. If found: the previous connection's state is migrated to the new
>    connection.

But the plan's Commit 3 only handles reconnection in the `WorkerStatus`
message path — it does not implement the `handle_connection`-side migration
of in-flight build state from the old connection_id to the new one.

Race condition: worker reconnects, grace-period monitor fires before
`WorkerStatus` is received, calls `handle_worker_dead` on the old
`connection_id`, finds the in-flight build still assigned to the old
connection, and fails it. The build is lost even though the worker is
alive and reconnected.

**Fix:** In `handle_connection`, after receiving a valid `Hello` and
before registering the new connection, scan `BuildQueue.workers` for a
`Disconnected` entry whose `registered_worker_id` matches. If found:
migrate active build `connection_id` references from old to new, remove
the old entry, register the new. This must be atomic under a single
queue lock.

### C3 — Force-disconnect does not clean up `worker_senders`

The force-disconnect path removes from `BuildQueue.workers` and calls
`handle_worker_dead`, but the plan doesn't specify explicitly that
`connection_id` is removed from `state.worker_senders`. Without this:

- The `UnboundedSender` is not dropped, so the forward task continues
  running, the socket stays open, and the worker keeps operating with
  a revoked key.
- Or if the implementer drops the sender some other way, `cleanup_worker`
  fires and tries to remove from `worker_senders` again (harmless no-op,
  but confusing).

**Fix:** Make explicit in the plan: "Remove `connection_id` from
`state.worker_senders` (step 3 in the corrected C1 sequence). This drops
the `UnboundedSender`, closes `outbound_rx`, triggers the forward task
exit, and ultimately `cleanup_worker`. Since the `BuildQueue.workers`
entry was already removed, `cleanup_worker` bails on the queue side."

---

## Major Concerns

### M1 — `handle_worker_dead` re-queues with hardcoded `Priority::Normal`

The existing `handle_worker_dead` re-queues in-flight builds with
`Priority::Normal` instead of `ab.priority`. This is a pre-existing bug
that Phase 7 exercises more frequently: deregistration of a connected
worker is the first intentional (not crash-recovery) path that re-queues
builds. A high-priority build force-re-queued silently becomes Normal.

`ActiveBuild` already carries `priority`. The fix is one line per field:
use `ab.priority`, `ab.user_email`, `ab.queued_at`.

**Fix:** Address in Commit 3 since it touches `handle_worker_dead` and
the reconnection path.

### M2 — `stopping` state missing from Commit 6 merge algorithm

The plan's Commit 6 algorithm says: "set `status` based on `WorkerState`
(`connected`, `building`, `disconnected`)" but the `WorkerInfo.status`
contract includes `"stopping"`. A `Stopping` worker falls through to
`"offline"` in the current narration.

**Fix:** Add explicit mapping: `WorkerState::Stopping` → `"stopping"`.

### M3 — Arch validation duplicated in three places

Arch values are validated in: (1) SQL CHECK constraint, (2) registration
handler, (3) `ServerConfig::validate()` for seed entries. Adding a new
arch requires updating all three.

**Fix:** Use the `Arch` enum's `FromStr`/serde impl as the single
validation source. `SeedWorker.arch` should be typed as `Arch` directly.
Handler validation parses via the same enum. The SQL CHECK is the DB-level
safety net.

---

## Minor Issues

- **`builder` role does not include `workers:view`.** The `viewer` role
  has it, but `builder` is independent. A user with only `builder` cannot
  see workers. Verify this is intentional.
- **`WorkerRow.arch` is `String` not `Arch`.** The Commit 3 arch
  validation requires parsing the DB string into `Arch`. Either use
  `Arch` in `WorkerRow` with custom decode, or add explicit parse with
  clear error.
- **`WorkerToken.arch` is `String` for forward-compatibility.** Acceptable
  but `Arch` is strictly safer since token arch values can only be those
  the server accepted at registration.
- **`last_insert_rowid()` is connection-scoped.** Use
  `result.last_insert_rowid()` from the `Execute` result on the same
  `tx`, not a separate query.
- **`builds.worker_id` semantic change.** Migration comment present.
  Adequate for pre-release.
- **201 response body contains plaintext API key.** Ensure no response-
  body logging middleware captures this. Add a comment to the handler.
- **`last_seen` during long builds.** Updated on handshake and
  `build_finished`, not during a 4-hour build. Document as "last
  activity" not "liveness heartbeat" so operators interpret correctly.
- **`ServerMessage::Error` with `min_version: None` for arch mismatch.**
  Semantically correct. If message types are extended later, consider a
  dedicated `Rejected` variant.

---

## Strengths

- **All 7 prior blockers resolved with correct, concrete fixes.**
- **Transaction ordering is crash-safe.** Insert new → update FK → revoke
  old means any crash leaves at least one valid key.
- **`revoke_api_key_by_id` correctly bypasses owner filter.** Any admin
  can deregister any worker.
- **Worker key isolation via `worker:` prefix + filtered listing.**
  Prevents accidental self-service deletion.
- **Deferred force-disconnect to Commit 3 is honestly documented** with
  safe fallback behavior (key revoked, cache purged, worker disconnects
  on next auth check).
- **Argon2 consistently outside transactions** in all three sites
  (register, regenerate, seed). Also fixes pre-existing `seed.rs` bug.
- **Reconnection via crypto proof** (API key → worker UUID) is strictly
  superior to self-reported `worker_id` string.
- **`last_seen` on `build_finished`** is a clean proof-of-life addition.
- **Commit dependency graph is accurate.** Commits 4/5/6 correctly
  independent of each other.

---

## Open Questions

1. **After Commit 2, deregistered worker still sends messages until next
   auth check.** If it sends `build_finished` for a deleted worker row,
   does `update_last_seen` fail or silently return 0 rows? Make it
   return `Result<bool>` and treat 0 as acceptable.
2. **Double-connect before first cleanup?** Can two connections exist
   simultaneously for the same `registered_worker_id`? The queue map is
   keyed by `connection_id` (unique), so yes — two entries with different
   connection UUIDs but same `registered_worker_id`. The reconnect scan
   in `handle_connection` must handle finding multiple entries.
3. **`api_key_id → worker_row` lookup caching?** Currently uncached DB
   query per WS upgrade. Acceptable at single-digit fleet scale.
4. **Arch CHECK constraint list expected to be stable?** Adding a new arch
   later requires a migration. Fine for current scope.
