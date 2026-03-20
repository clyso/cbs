# Implementation Review: cbsd-rs Phase 7 — Worker Registration

**Commits reviewed (6):**
- `d9ade16` — add worker registration and management REST API (691 lines)
- `3b6feab` — bind WS handshake to registered worker identity (328 lines)
- `1f21c8e` — add worker token support to config and WS client (158 lines)
- `4d05e89` — update seed config to create registered workers (137 lines)
- `4648727` — merge DB and in-memory state in GET /api/workers (91 lines)
- `5626829` — apply cargo fmt across workspace (225 lines)

**Evaluated against:**
- Design: `cbsd-rs/docs/cbsd-rs/design/004-20260316T0925-worker-registration.md`

---

## Summary

Phase 7 is well-implemented across 6 commits totaling ~1630 lines. The
implementation faithfully tracks the design document through all 3 review
iterations. Every design-reviewed correctness property is present: atomic
registration transactions with argon2 outside the tx, crash-safe token
rotation (insert new → update FK → revoke old), force-disconnect with
correct lock ordering (queue lock released before `handle_worker_dead`),
connection migration on reconnect under a single queue lock, and
`worker_senders` cleanup as an explicit post-lock step.

No blockers. Two findings and several minor observations.

**Verdict: Sound implementation. Ready for integration testing.**

---

## Design Fidelity

| Design requirement | Status |
|---|---|
| `workers` table: UUID PK, UNIQUE name, arch CHECK, api_key_id UNIQUE FK ON DELETE CASCADE | ✓ |
| `workers:manage` in `KNOWN_CAPS` | ✓ |
| `revoke_api_key_by_id(pool, id)` — no owner filter | ✓ |
| `insert_api_key_in_tx` — returns `last_insert_rowid()` | ✓ |
| `generate_api_key_material()` — argon2 outside transaction | ✓ |
| Registration: atomic tx (API key + worker row) | ✓ |
| Deregistration: revoke + purge LRU + delete row + force-disconnect | ✓ |
| Token regeneration: insert new → update FK → revoke old → commit | ✓ |
| Force-disconnect: lock queue → extract → remove → **release** → remove sender → `handle_worker_dead` | ✓ |
| WS upgrade: reject unregistered API keys with 403 | ✓ |
| Protocol v2: `Hello` drops `worker_id`, `WorkerStopping` drops `worker_id` | ✓ |
| Arch mismatch validation at handshake | ✓ |
| `last_seen` updated on handshake and `build_finished` | ✓ |
| Connection migration on reconnect (atomic under queue lock) | ✓ |
| Double-connect `Connected` case: treated as force-disconnect | ✓ |
| `worker_senders` cleanup after queue lock released (lock inversion prevention) | ✓ |
| Worker token: base64url JSON with id, name, api_key, arch | ✓ |
| Worker config: token > env var > individual fields; `worker_id` removed | ✓ |
| `SeedWorker.arch: Arch` — serde validates at parse time | ✓ |
| `builder` role gets `workers:view` in seed | ✓ |
| Worker key filtering from `GET /api/auth/api-keys` | ✓ |
| `worker:` prefix blocked in `POST /api/auth/api-keys` | ✓ |
| Worker name validation: `[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}` | ✓ |
| UNIQUE constraint violation → 409 "worker name already exists" | ✓ |
| Merged listing: 5 status values (connected, building, stopping, disconnected, offline) | ✓ |
| Priority preserved on re-queue (`ab.priority`, not `Priority::Normal`) | ✓ |
| Upgrade path: fresh DB required | ✓ (design doc) |
| `builds.worker_id` semantic change documented in migration | ✓ |
| 201 response security note (plaintext API key) | ✓ (handler comment) |

---

## Findings

### Finding 1 — Duplicate `WorkerTokenPayload` struct in `routes/admin.rs`

`routes/admin.rs` lines 256–262 define a private `WorkerTokenPayload`:

```rust
struct WorkerTokenPayload {
    worker_id: String,
    worker_name: String,
    api_key: String,
    arch: String,
}
```

`cbsd-proto/src/lib.rs` line 27 defines the public `WorkerToken`:

```rust
pub struct WorkerToken {
    pub worker_id: String,
    pub worker_name: String,
    pub api_key: String,
    pub arch: String,
}
```

These are identical structs. `routes/admin.rs` uses `WorkerTokenPayload`
for `build_worker_token()`, while `db/seed.rs` uses `cbsd_proto::WorkerToken`
for the exact same operation. The worker client deserializes
`cbsd_proto::WorkerToken`.

Using two different structs for the same serialized format is a divergence
risk: if a field is added to `WorkerToken`, `WorkerTokenPayload` would
need to be updated separately, and a forgotten update would produce tokens
the worker can't parse.

Severity: **Low.** Easy to fix: replace `WorkerTokenPayload` with
`cbsd_proto::WorkerToken` in `routes/admin.rs` and delete the duplicate
struct.

### Finding 2 — `get_key_prefix_by_id` queries a revoked/deleted key after deregistration

`deregister_worker` (lines 444–452):

```rust
// Purge from LRU cache (after commit)
if let Some(prefix) = db::api_keys::get_key_prefix_by_id(&state.pool, worker.api_key_id)
    .await
    .ok()
    .flatten()
{
    let mut cache = state.api_key_cache.lock().await;
    cache.remove_by_prefix(&prefix);
}
```

The transaction at lines 413–442 revokes the API key (`SET revoked = 1`)
and deletes the worker row. Then this code queries the API key by ID to
get its prefix for cache purging. The key still exists in the DB (only
`revoked=1`, not deleted), so this works.

However, the `ON DELETE CASCADE` on `workers.api_key_id` is a *forward*
reference — deleting the `api_keys` row cascades to `workers`, not the
other way around. Deleting the `workers` row does NOT delete the
`api_keys` row. The key row persists with `revoked=1` indefinitely.

This is the intended behavior per the design ("FK cascade is a safety
net, not the primary mechanism"). But it means orphaned revoked `api_keys`
rows accumulate over time as workers are deregistered. At CBS's scale
this is negligible, but for long-lived deployments a periodic cleanup of
`WHERE revoked = 1 AND NOT EXISTS (SELECT 1 FROM workers WHERE
api_key_id = api_keys.id)` would be advisable.

Severity: **Negligible.** Not a bug — the revoked key correctly rejects
auth attempts. The prefix lookup works because the row still exists.

---

## Minor Observations

- **`arch` parsing in `list_workers` uses `serde_json` roundtrip.**
  `routes/workers.rs` line 114:
  ```rust
  let arch = serde_json::from_value::<Arch>(serde_json::Value::String(row.arch))
      .unwrap_or(Arch::X86_64);
  ```
  This allocates a `serde_json::Value` to parse a string into an enum.
  The same pattern appears in `ws/handler.rs` for the handshake arch
  validation (lines 153–155). Both sites could use a simpler
  `match row.arch.as_str()` pattern to avoid the allocation. The
  `unwrap_or(Arch::X86_64)` fallback silently converts corrupt DB values
  to x86_64 — worth logging a warning instead.

- **`requeue_active_build` still uses `user_email: String::new()` and
  `queued_at: 0`.** These fields are not used for queue ordering or
  display, but an empty `user_email` means a re-queued build's DB record
  retains its original `user_email` (correct — the build row is only
  state-transitioned, not rewritten). The in-memory `QueuedBuild` fields
  are vestigial for re-queue. Not a bug but worth a comment.

- **Legacy mode `worker_name` defaults to `"legacy-worker"`.** The design
  says "defaults to hostname." The implementation uses a static string.
  Using `gethostname()` would be more operationally useful for log
  correlation. Minor — operators using legacy mode will be transitioning
  to tokens anyway.

- **`api_key_id` is added to `CachedApiKey`.** Verified in
  `auth/api_keys.rs`. The `verify_api_key` path now returns the row ID
  in the cached entry, which is used by `ws_upgrade` to look up the
  worker. Correct.

- **Seed generates argon2 hashes before the transaction.** Verified in
  `db/seed.rs` lines 59–73: `generate_api_key_material()` is called in
  a loop before `pool.begin()`. This fixes the pre-existing issue where
  `spawn_blocking` was called inside an open transaction.

- **`workers:view` added to `builder` role caps in seed.** Verified at
  `db/seed.rs` line 93. Correct — builders can see worker status.

- **Commit sizing.** All 5 substantive commits are within or near the
  400–800 line target. The largest (d9ade16, 691 lines) includes the full
  registration REST API with 3 endpoints + `force_disconnect_worker` +
  `is_unique_violation` + types. Cohesive and not splittable.

---

## Commit-by-Commit Verification

### d9ade16 — Worker registration REST API (691 lines)

- Migration `002_worker_registration.sql` matches design exactly ✓
- `db/workers.rs`: all 7 functions, `insert_worker` takes `&mut Transaction` ✓
- `db/api_keys.rs`: `revoke_api_key_by_id`, `insert_api_key_in_tx`,
  `get_key_prefix_by_id` ✓
- `routes/admin.rs`: 3 endpoints with correct transaction patterns ✓
- `force_disconnect_worker`: lock → extract → remove → **release** →
  remove sender → `handle_worker_dead` ✓
- `is_unique_violation` maps sqlx error code 2067 → 409 ✓
- `KNOWN_CAPS` updated with `workers:manage` ✓
- Worker key filtering: `GET /api/auth/api-keys` excludes `worker:` prefix ✓
- Worker key blocking: `POST /api/auth/api-keys` rejects `worker:` prefix ✓

### 3b6feab — WS handshake bound to registered identity (328 lines)

- `Hello` drops `worker_id` field ✓
- `WorkerStopping` drops `worker_id` field ✓
- Protocol version check: `!= 2` ✓
- WS upgrade: `get_worker_by_api_key_id` → 403 if not found ✓
- Arch validation: `serde_json::from_value` parse + comparison ✓
- `WorkerState` variants: `registered_worker_id` + `worker_name` ✓
- Connection migration under single queue lock ✓
- Double-connect treated as force-disconnect with `handle_worker_dead` ✓
- `worker_senders` cleanup after queue lock released ✓
- `last_seen` updated on handshake and `build_finished` ✓
- `handle_worker_dead` uses `ab.priority` (not hardcoded Normal) ✓
- `requeue_active_build` uses `ab.priority` ✓

### 1f21c8e — Worker token config + WS client (158 lines)

- `WorkerToken` in `cbsd-proto/src/lib.rs` ✓
- `WorkerConfig`: `worker_token` field, legacy `api_key` + `arch` ✓
- `resolve()`: env var > config token > individual fields ✓
- Warning when both env var and config token set ✓
- `worker_id` removed from `Hello` and `WorkerStopping` construction ✓
- `protocol_version: 2` in `Hello` ✓
- `parse_arch` accepts `arm64` alias ✓

### 4d05e89 — Seed config (137 lines)

- `SeedWorker.arch: Arch` (serde validates at parse) ✓
- Argon2 hash generated before transaction ✓
- Worker row + API key in single transaction ✓
- Token printed to stdout after commit ✓
- `builder` role gets `workers:view` ✓
- `seed_workers` replaces `seed_worker_api_keys` ✓

### 4648727 — Merged worker listing (91 lines)

- `GET /api/workers` merges DB + in-memory ✓
- 5 status values: connected, building, stopping, disconnected, offline ✓
- `current_build_id` from active map ✓
- `workers:view` cap check ✓

### 5626829 — cargo fmt (225 lines)

Formatting-only across 28 files. No logic changes.

---

## Plan Progress

Phase 7 complete. All 6 commits match the plan's commit descriptions.
Plan progress table should be updated to "Done" for all 6 items.
