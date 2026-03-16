# Design & Plan Review: Worker Registration (Phase 7)

**Documents reviewed:**
- `_docs/cbsd-rs/design/2026-03-16-worker-registration.md`
- `_docs/cbsd-rs/plans/phase-7-worker-registration.md`

**Cross-referenced against:**
- Design README, arch doc, auth doc
- Current implementation: `ws.rs`, `permissions.rs`, `config.rs`,
  `seed.rs`, `liveness.rs`, `queue/mod.rs`

---

## Summary

The design is well-scoped and closes three real weaknesses: no persistent
worker identity, API keys not bound to workers, and bootstrap friction.
The data model (UUID-keyed `workers` table with one-to-one FK to
`api_keys`), the token-based bootstrap mechanism, and the protocol v2
changes are all sound.

Seven issues must be resolved before implementation begins. The most
critical are: the deregistration handler targets the wrong API key (uses
`revoke_api_key_by_prefix(owner_email, prefix)` but the calling admin may
not be the original registrant), and the `regenerate-token` handler has a
non-atomic revoke→create→update sequence where a crash between steps
leaves the worker permanently bricked. Additionally, the registration
handler is not atomic as specified, `workers:manage` is missing from
`KNOWN_CAPS`, and the plan has a Commit 2/3 cross-dependency on the
`WorkerState` struct.

**Verdict: Revise and re-review.** The core design is solid; these are
implementation-level issues with clear fixes.

---

## Blockers

### B1 — Registration handler is not transactional

The plan's Commit 2 calls `create_api_key(pool, ...)` (auto-commits),
then queries for the key's row ID by prefix, then inserts the `workers`
row. A crash between the key insert and the worker insert produces an
orphaned, unrevoked API key with no corresponding worker row.

**Fix:** Wrap all three operations in a single transaction, following the
`generate_api_key_in_tx` pattern from `seed.rs`. Use
`last_insert_rowid()` within the transaction to get the key's row ID —
no second query needed. Change `create_api_key` to return
`(plaintext, prefix, row_id)` or add an in-transaction variant.

### B2 — `workers:manage` missing from `KNOWN_CAPS`

`permissions.rs` rejects unknown capability strings with 400. The design
introduces `workers:manage` but the plan does not add it to `KNOWN_CAPS`.
Any attempt to create a custom role with `workers:manage` will fail.

**Fix:** Add `"workers:manage"` to `KNOWN_CAPS` in Commit 2.

### B3 — `deregister_worker` uses `revoke_api_key_by_prefix(owner_email, ...)` — wrong admin → no-op

The existing `revoke_api_key_by_prefix` filters by `owner_email`. The
calling admin may not be the admin who originally registered the worker.
If the emails differ: `rows_affected() == 0`, key is NOT revoked, handler
continues to delete the worker row, leaving an active unrevoked key.

**Fix:** Add `revoke_api_key_by_id(pool, api_key_id)` that revokes by
primary key, bypassing the owner filter. The `workers.api_key_id` FK
gives the row ID directly. Use in both `deregister` and `regenerate`.

### B4 — `regenerate-token` is not atomic — crash between revoke and update bricks the worker

Steps: (1) revoke old key, (2) create new key, (3) update
`workers.api_key_id`. A crash after (1) but before (3) leaves the worker
with a revoked key and no replacement — permanently locked out until
manual DB intervention. Additionally, the live WS connection is not
closed (key was revoked but the session was authenticated at upgrade time).

**Fix:** Wrap in a single transaction: insert new key → update
`workers.api_key_id` → revoke old key → commit. LRU cache purge after
commit. Explicitly close the active WS connection after the transaction
succeeds.

### B5 — `WorkerState` field changes span Commits 2 and 3 with a compile dependency

Commit 2's `deregister_worker` needs to scan `BuildQueue.workers` to
find the connection for the deregistered worker. The plan scans by
`registered_worker_id`, which is added in Commit 3. Commit 2 code using
the old `worker_id` field compiles, but Commit 3 renames it —
effectively making both commits co-dependent.

**Fix:** Either (a) have Commit 2's deregister scan by matching the
`active` map's connection_id (avoiding the `WorkerState` field
entirely), or (b) move the `WorkerState` field additions to Commit 2.

### B6 — Seed transaction calls `spawn_blocking` (argon2) while holding a sqlx transaction

The existing `generate_api_key_in_tx` and the plan's Commit 5 both run
argon2 via `spawn_blocking` inside an open transaction. This holds a pool
connection while yielding to the executor. Under pool pressure
(`max_connections = 4`), the subsequent `execute(&mut *tx)` can deadlock.

**Fix:** Generate the argon2 hash *before* opening the transaction (hash
requires no DB state). Match the production `create_api_key()` pattern
which hashes first, then inserts.

### B7 — Legacy mode breaks upgrade path for non-fresh deployments

After Commit 3, unregistered API keys are rejected at WS upgrade (403).
Existing workers seeded with `seed_worker_api_keys` (no `workers` table
row) will be immediately locked out on upgrade. The plan assumes fresh
deployments only but doesn't mention this constraint.

**Fix:** Either (a) provide a one-time migration script that creates
`workers` rows for existing seeded API keys (run before server upgrade),
or (b) explicitly document that Phase 7 requires a fresh DB deployment.
Option (a) is more robust.

---

## Major Concerns

### M1 — `GET /api/workers` vs `GET /api/admin/workers` path contradiction

The design doc puts the new listing at `GET /api/admin/workers`. The
plan's Commit 6 says the route stays at `GET /api/workers`. These are the
same endpoint being updated in-place, but the design and plan use
different path names in different sections.

**Fix:** Consolidate to one path. `GET /api/workers` with `workers:view`
is the existing and simpler choice. Update the design doc's API table.

### M2 — `last_seen` updated only on handshake, not on activity

A worker connected for 30 days without reconnecting shows `last_seen`
from 30 days ago. The migration comment says "updated on WS
connect/heartbeat" but no heartbeat mechanism exists. Misleading for
operators identifying stale connections.

**Fix:** Either (a) update `last_seen` on `build_finished` (cheap,
proves the worker did something), or (b) change the migration comment
and API docs to `"last connection time"` so operators interpret it
correctly.

### M3 — `ON DELETE CASCADE` missing from plan's migration DDL

The design doc specifies `api_key_id ... REFERENCES api_keys(id) ON
DELETE CASCADE`. The plan's Commit 1 DDL omits `ON DELETE CASCADE`. The
design describes the cascade as a safety net — without it, direct API key
deletion produces a FK constraint violation instead of cleanup.

**Fix:** Add `ON DELETE CASCADE` to the plan's migration DDL.

### M4 — `deregister_worker` double-invokes `handle_worker_dead`

Deregistration sets `WorkerState::Dead` then drops the WS sender channel.
The channel drop triggers `cleanup_worker` which calls
`handle_worker_dead` again. The second call is a no-op (empty `active`
map), but the pattern is fragile.

**Fix:** Remove the connection from `workers` entirely (not just set
Dead) before dropping the channel, so `cleanup_worker` finds no entry
and bails. Or document the double-invocation as expected and safe.

---

## Minor Issues

- **Worker name validation regex unspecified.** Plan says "alphanumeric +
  hyphens" but no CHECK constraint in SQL. Should underscores be allowed?
  Specify precisely or add a CHECK constraint.
- **`created_by` FK on `users.email` has no `ON DELETE` clause.** If the
  admin is later deleted (not just deactivated), FK violation. Document
  the assumption that users are only deactivated.
- **`WorkerToken.arch` is `String` not `Arch`.** Since `Arch` with serde
  aliases exists in `cbsd-proto`, using it directly avoids a re-parse
  step in the worker's `resolve()`. But `String` is more forward-compatible
  with new arch values.
- **Seed workers: `SeedWorker.arch: String` not validated at config load.**
  A typo like `arch: armm64` would fail at the DB CHECK constraint inside
  the seed transaction rather than at startup validation. Add arch
  validation to `ServerConfig::validate()`.
- **`cargo sqlx prepare` not explicitly called out for Commit 1.** The
  CLAUDE.md rule says "after any commit that adds sqlx queries, re-run."
  Commit 1 adds a migration and Commits 2/3 add queries.
- **Worker API keys appear in the admin's `GET /auth/api-keys` listing.**
  They're prefixed with `worker:` but the admin could accidentally delete
  one via `DELETE /auth/api-keys/{prefix}`. Consider filtering `worker:`
  keys from the self-service listing.
- **`Hello.arch` mismatch error should use `min_version: None,
  max_version: None`** (not version numbers — it's an arch error, not a
  version error).
- **`Stopping` state missing from design's status enum.** The design lists
  `connected`, `building`, `disconnected`, `offline`. The existing
  `WorkerState::Stopping` variant maps to `"stopping"`. Specify how it
  maps in the merged listing.
- **`builds.worker_id` semantic change** from display label to UUID is
  undocumented in the migration. Old records won't join. Add a comment.
- **`workers:view` added to `builder` role is seed-time only.** Existing
  deployments with a `builder` role won't get the cap retroactively. Fine
  for pre-release but document the scope.

---

## Suggestions

- **Return `(plaintext, prefix, row_id)` from `create_api_key`.** Use
  `RETURNING id` or `last_insert_rowid()`. Eliminates the lookup-by-prefix
  step in both registration and seed paths.
- **Update `last_seen` on `build_finished`.** Cheap proof-of-life that
  keeps the field useful for fleet monitoring without per-heartbeat DB
  writes.
- **Add `"stopping"` to the merged listing status enum.** Or map it to
  `"disconnected"` — but be explicit.
- **Consider `revoke_api_key_by_id(pool, id)` as a general-purpose
  function** alongside the existing `revoke_api_key_by_prefix`. Both the
  deregistration and regeneration handlers need it.
- **Document the lost-token recovery workflow:** deregister → re-register
  → new token. This is implied but should be explicit in the design or
  an operator runbook.

---

## Strengths

- **Token-based bootstrap is elegant.** Base64url JSON with
  `{worker_id, worker_name, api_key, arch}`, accepted via config file or
  env var. `server_url` correctly excluded (deployment-specific).
- **One-to-one key binding at the DB layer.** `UNIQUE` FK on `api_key_id`
  enforces the invariant structurally, not just in application code.
- **Protocol v2 handled cleanly.** Explicit version bump, v1 rejection
  with clear error, pre-release context acknowledged.
- **Reconnection via crypto proof.** API key → worker UUID lookup replaces
  the fragile self-reported `worker_id` string match.
- **Arch validation at handshake.** Catches token-copy errors between
  different-architecture machines with an actionable error message.
- **`builds.worker_id` stores UUID.** Enables proper FK joins for fleet
  analytics. Old records gracefully degrade (no join, acceptable).
- **Dependency graph is correct.** Commit 1 → 2 → 3 → [4, 5, 6] with
  independence of the leaf commits clearly noted.

---

## Open Questions

1. **Deregistration of a `Disconnected` worker (in grace period)?** The
   grace period monitor fires after deregistration and calls
   `handle_worker_dead` on a connection with no valid worker row.
   Specify the interaction.
2. **Worker API keys in admin's `GET /auth/api-keys` listing?** Should
   `worker:`-prefixed keys be filtered from the self-service listing to
   prevent accidental deletion?
3. **Lost token recovery workflow?** Deregister → re-register → new
   token? Document explicitly.
4. **`SeedWorker.arch` validation at startup?** Invalid arch string
   should panic at config load, not at seed transaction time.
5. **`WorkerStopping` logging on the worker side after `worker_id`
   removal?** The worker currently logs its own `worker_id` from the
   message it constructs. After v2, it uses `worker_name` from the
   resolved config. Is this covered in the plan's Commit 4 changes?
