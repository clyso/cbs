# Phase 7: Worker Registration

**Design document:** `cbsd-rs/docs/cbsd-rs/design/004-20260316T0925-worker-registration.md`

## Progress

| # | Commit | ~LOC | Status |
|---|--------|------|--------|
| 1 | `server: add worker registration and management REST API` | ~620 | Done |
| 2 | `server: bind WS handshake to registered worker identity` | ~330 | Done |
| 3 | `worker: add worker token support to config and WS client` | ~160 | Done |
| 4 | `server: update seed config to create registered workers` | ~140 | Done |
| 5 | `server: update GET /api/workers to merge DB and in-memory state` | ~90 | Done |

**Total:** ~1340 LOC across 5 commits (+1 formatting cleanup).

---

## Commit 1: `server: add worker registration and management REST API`

Migration, DB queries, and route handlers form a single logical change:
the DB functions exist solely to serve the route handlers and have no
independent callers or tests without them.

**Files:**

- `cbsd-rs/migrations/002_worker_registration.sql` (new)
- `cbsd-rs/cbsd-server/src/db/workers.rs` (new)
- `cbsd-rs/cbsd-server/src/db/api_keys.rs` (add `revoke_api_key_by_id`,
  `insert_api_key_in_tx`)
- `cbsd-rs/cbsd-server/src/db/mod.rs` (add `pub mod workers`)
- `cbsd-rs/cbsd-server/src/routes/admin.rs` (add registration endpoints)
- `cbsd-rs/cbsd-server/src/routes/auth.rs` (filter `worker:` keys from
  `GET /api/auth/api-keys`)
- `cbsd-rs/cbsd-server/src/routes/permissions.rs` (add `workers:manage` to
  `KNOWN_CAPS`)
- `cbsd-rs/cbsd-server/src/app.rs` (add route nesting)
- `cbsd-rs/.sqlx/` (regenerated)

**Content:**

### Migration `002_worker_registration.sql`

```sql
-- Worker registration: persistent worker identity bound to API keys.
-- Note: builds.worker_id changes semantics from display label to UUID
-- after this migration. Old records retain their original display labels.
CREATE TABLE IF NOT EXISTS workers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    arch        TEXT NOT NULL CHECK (arch IN ('x86_64', 'aarch64')),
    api_key_id  INTEGER NOT NULL UNIQUE
                REFERENCES api_keys(id) ON DELETE CASCADE,
    created_by  TEXT NOT NULL REFERENCES users(email),
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen   INTEGER
);
```

### DB module `db/workers.rs`

Functions:

- `insert_worker(tx, id, name, arch, api_key_id, created_by) -> Result`
  (takes `&mut Transaction`, not pool — used inside registration tx)
- `get_worker_by_id(pool, id) -> Result<Option<WorkerRow>>`
- `get_worker_by_api_key_id(pool, api_key_id) -> Result<Option<WorkerRow>>`
- `list_workers(pool) -> Result<Vec<WorkerRow>>`
- `delete_worker(pool, id) -> Result<bool>`
- `update_last_seen(pool, id) -> Result<()>`
- `update_api_key_id(tx, id, new_api_key_id) -> Result<()>` (takes tx)

The `WorkerRow` struct:

```rust
pub struct WorkerRow {
    pub id: String,
    pub name: String,
    pub arch: String,
    pub api_key_id: i64,
    pub created_by: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
}
```

### DB module `db/api_keys.rs` additions

- `revoke_api_key_by_id(pool, api_key_id) -> Result<bool>` — revokes by
  primary key, no `owner_email` filter. Used by deregister and regenerate.
- `insert_api_key_in_tx(tx, name, owner_email, key_hash, key_prefix)
  -> Result<i64>` — inserts within a transaction, returns
  `result.last_insert_rowid()`.

### `POST /api/admin/workers`

Handler `register_worker`:

1. Require `workers:manage` cap.
2. Validate `name` matches `[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}`.
3. Validate `arch` is `x86_64` or `aarch64`.
4. Generate worker UUID v4.
5. Generate random API key bytes and argon2 hash **before** the transaction
   (CPU-bound work must not hold a pool connection).
6. Begin transaction.
7. Insert API key row via `insert_api_key_in_tx(tx, "worker:<name>",
   user.email, hash, prefix)`. Use `result.last_insert_rowid()` →
   `api_key_id`. Map `UniqueViolation` to 409 `"worker name already exists"`.
8. Insert `workers` row via `insert_worker(tx, uuid, name, arch, api_key_id,
   user.email)`.
9. Commit transaction.
10. Build the worker token JSON: `{"worker_id", "worker_name", "api_key",
    "arch"}`.
11. Base64url-encode (no padding).
12. Return 201 with `{worker_id, name, arch, worker_token}`.
    Note: response contains plaintext API key — add a handler comment
    warning against response-body logging.

### `DELETE /api/admin/workers/{id}`

Handler `deregister_worker`:

1. Require `workers:manage` cap.
2. Look up worker row by ID → get `api_key_id`, `name`.
3. Revoke API key by ID: `revoke_api_key_by_id(pool, api_key_id)` — no
   `owner_email` filter, any admin can deregister any worker.
4. Purge from LRU cache by looking up the key prefix.
5. Delete the worker row.
6. **Force-disconnect deferred to Commit 2** — after this commit, the worker
   stays connected until its next auth check fails (key revoked, cache
   purged). Commit 2 adds `registered_worker_id` to `WorkerState` and
   implements the remove-from-map → remove-sender → `handle_worker_dead`
   sequence.
7. Return 200.

### `POST /api/admin/workers/{id}/regenerate-token`

Handler `regenerate_worker_token`:

1. Require `workers:manage` cap.
2. Look up worker row → get old `api_key_id`.
3. Generate new API key bytes and argon2 hash **before** the transaction.
4. Begin transaction.
5. Insert new API key row → get new `api_key_id`.
6. Update `workers.api_key_id` to the new key.
7. Revoke old API key by ID.
8. Commit transaction.
9. Purge old API key from LRU cache.
10. Build and return new worker token.

The transaction ordering (insert new → update FK → revoke old) ensures no
crash window leaves the worker bricked. Pre-commit: old key valid. Post-
commit: new key active.

**Force-disconnect on regenerate also deferred to Commit 2** (same reasoning
as deregister — needs `registered_worker_id` on `WorkerState`).

### `KNOWN_CAPS` update

Add `"workers:manage"` to the `KNOWN_CAPS` list in `permissions.rs`. Without
this, creating a custom role with `workers:manage` would fail with 400.

### Worker key filtering

In `routes/auth.rs`, update `list_api_keys_handler` to filter out keys whose
name starts with `worker:`. This prevents users from accidentally deleting
worker keys via `DELETE /api/auth/api-keys/{prefix}`.

### Route nesting

Add the three endpoints to the existing `admin::router()` under the
`/workers` prefix:

```rust
.route("/workers", post(register_worker))
.route("/workers/{id}", delete(deregister_worker))
.route("/workers/{id}/regenerate-token", post(regenerate_worker_token))
```

**Testable after:** `cargo build --workspace`. `cargo sqlx prepare`. Can
manually test registration, deregistration, and token regeneration with
`curl`.

---

## Commit 2: `server: bind WS handshake to registered worker identity`

**Files:**

- `cbsd-rs/cbsd-server/src/ws/handler.rs` (modify `ws_upgrade`,
  `handle_connection`, `build_finished` handler for `last_seen` update)
- `cbsd-rs/cbsd-server/src/auth/api_keys.rs` (add `api_key_id` to
  `CachedApiKey`)
- `cbsd-rs/cbsd-server/src/db/api_keys.rs` (return `id` in verification path)
- `cbsd-rs/cbsd-server/src/ws/liveness.rs` (replace `worker_id` with
  `registered_worker_id` and `worker_name`)
- `cbsd-rs/cbsd-server/src/queue/mod.rs` (update `WorkerInfo`)
- `cbsd-rs/cbsd-server/src/routes/admin.rs` (add force-disconnect to
  deregister and regenerate handlers)
- `cbsd-rs/cbsd-proto/src/ws.rs` (remove `worker_id` from `Hello` and
  `WorkerStopping`, bump protocol version to 2)
- `cbsd-rs/.sqlx/` (regenerated)

**Content:**

### Protocol v2: drop `worker_id` from `Hello` and `WorkerStopping`

In `cbsd-proto/src/ws.rs`:

```rust
// Before (v1):
Hello { protocol_version, worker_id, arch, cores_total, ram_total_mb }
WorkerStopping { worker_id, reason }

// After (v2):
Hello { protocol_version, arch, cores_total, ram_total_mb }
WorkerStopping { reason }
```

The server identity for the worker is now derived from the API key at
WS upgrade, not self-reported. The `worker_id` field is removed entirely.

Update the protocol version check from `!= 1` to `!= 2`.

### API key cache: add `api_key_id`

Add `api_key_id: i64` to `CachedApiKey`. This is populated during
verification from the DB row and cached for subsequent lookups. The
`ApiKeyRow` already has `id: i64`.

### WS upgrade: look up registered worker

After `verify_api_key()` succeeds in `ws_upgrade`:

1. Get `api_key_id` from the returned `CachedApiKey`.
2. Call `db::workers::get_worker_by_api_key_id(pool, api_key_id)`.
3. If `None`: return 403 with `"API key is not bound to a registered worker"`.
4. If `Some(worker_row)`: pass the `WorkerRow` into `handle_connection` as a
   new parameter.

### Connection handler: use registered identity

In `handle_connection`:

- After receiving `Hello`, validate `Hello.arch` matches `worker_row.arch`.
  On mismatch: send `Error` with a clear message (e.g., `"arch mismatch:
  worker registered as x86_64 but reported aarch64 — re-register with
  correct arch or fix the worker token"`) and disconnect. Use
  `min_version: None, max_version: None` (arch error, not version
  negotiation). Parse `worker_row.arch` to `Arch` with clear error.
- Use `worker_row.id` as the canonical worker identity for all queue
  operations, build dispatch, and logging.
- Use `worker_row.name` for structured log fields (`worker_name`).
- Update `workers.last_seen`.

### Worker state: replace `worker_id` with registered identity

Replace `worker_id: String` in `WorkerState::Connected`, `Disconnected`,
and `Stopping` variants with:

- `registered_worker_id: String` — the UUID from `workers.id`
- `worker_name: String` — the human-readable name from `workers.name`

All existing code that referenced `worker_id()` on `WorkerState` is updated
to use `worker_name()` (for display) or `registered_worker_id()` (for
identity matching).

### Connection migration on reconnect

In `handle_connection`, after receiving a valid `Hello` and **before
registering the new connection**, scan `BuildQueue.workers` under a single
queue lock for any existing entry whose `registered_worker_id` matches:

- **If found and `Disconnected`:** Under the queue lock: migrate active
  build `connection_id` references from old to new, remove old entry,
  register the new connection. **After releasing the queue lock:** remove
  old `connection_id` from `worker_senders`. This is safe because the
  grace-period monitor needs the queue lock — it cannot fire during the
  window between lock release and `worker_senders` cleanup. The queue map
  operations (migrate, remove old, register new) are atomic under one lock;
  `worker_senders` is a separate mutex and must not be acquired while
  holding the queue lock (lock inversion: `cleanup_worker` acquires
  `worker_senders` first, then queue).

- **If found and `Connected`** (stale double-connect — should not happen
  normally): treat identically to force-disconnect. Under the queue lock:
  extract old `connection_id`, remove old entry, register new connection.
  After releasing the queue lock: remove old `connection_id` from
  `worker_senders`, call `handle_worker_dead(state, old_connection_id)` to
  re-queue any active build assigned to the old connection. Log a warning.

- **If not found:** register as fresh connection.

The queue lock is the synchronization boundary. All queue map mutations
(remove old, register new, migrate build references) happen atomically
under it. `worker_senders` cleanup and `handle_worker_dead` happen after
release, following the same lock-ordering discipline as force-disconnect.

This replaces the existing `WorkerStatus::Building` path for reconnection
matching, which used the self-reported `worker_id` string.

### `builds.worker_id`

Update `db::builds::mark_dispatched()` to store the worker's registered UUID
(from `worker_row.id`) instead of the self-reported `Hello.worker_id`.

### `last_seen` on `build_finished`

Update `workers.last_seen` when the server processes a `build_finished`
message. This provides a proof-of-life signal more useful than connection
time alone. Implemented in the `build_finished` handler in `ws/handler.rs`.
`update_last_seen` returns `Result<bool>` — 0 rows affected (e.g., worker
deregistered mid-build) is acceptable, not an error.

### Force-disconnect for deregister and regenerate

Now that `registered_worker_id` exists on `WorkerState`, add the force-
disconnect logic to the deregister and regenerate handlers in
`routes/admin.rs`. The sequence must avoid deadlock (`handle_worker_dead`
re-acquires the queue mutex, which is non-reentrant):

1. Lock `BuildQueue.workers` → scan for a connection whose
   `registered_worker_id` matches the target worker UUID → extract
   `connection_id` → remove entry from the map → **release lock.**
2. Remove `connection_id` from `state.worker_senders`. This drops the
   `UnboundedSender`, closing `outbound_rx`, which terminates the forward
   task and triggers `cleanup_worker`. Since the `BuildQueue.workers` entry
   was already removed, `cleanup_worker` finds nothing and bails.
3. Call `handle_worker_dead(state, connection_id)` to re-queue any in-flight
   build.

**The queue mutex must be released before step 3.** `handle_worker_dead`
calls `state.queue.lock().await` internally — holding the lock across this
call would deadlock.

### Fix: `handle_worker_dead` priority preservation

Pre-existing bug: `handle_worker_dead` re-queues in-flight builds with
hardcoded `Priority::Normal`. Fix to use `ab.priority` (the `ActiveBuild`
already carries the original priority). This commit touches
`handle_worker_dead` for the reconnection and force-disconnect paths, so
the priority fix belongs here.

### Disconnected/grace-period interaction

For `Disconnected` workers being deregistered: the grace-period monitor will
fire later, find no entry in the map, and bail harmlessly.

**Testable after:** `cargo build --workspace`. Workers without a registered
identity are rejected at WS upgrade. Workers with a registered identity
connect successfully and their UUID appears in build records. Arch mismatch
is rejected with a clear error. Deregistering a connected worker force-
disconnects it.

---

## Commit 3: `worker: add worker token support to config and WS client`

**Files:**

- `cbsd-rs/cbsd-worker/src/config.rs` (add `worker_token` + env var,
  restructure fields)
- `cbsd-rs/cbsd-worker/src/main.rs` (use resolved config)
- `cbsd-rs/cbsd-worker/src/ws/client.rs` (update `Hello` and
  `WorkerStopping` construction — drop `worker_id`)
- `cbsd-rs/cbsd-proto/src/lib.rs` (add `WorkerToken` struct)

**Content:**

### Worker token struct (in `cbsd-proto`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerToken {
    pub worker_id: String,
    pub worker_name: String,
    pub api_key: String,
    pub arch: String,
}
```

Lives in `cbsd-proto` since the server constructs it (serialize) and the
worker consumes it (deserialize).

### Config resolution

In `WorkerConfig`:

```rust
pub struct WorkerConfig {
    pub server_url: String,

    // Token-based config (preferred)
    #[serde(default)]
    pub worker_token: Option<String>,

    // Legacy individual fields (used when no token)
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,

    // ... rest unchanged (no more worker_id field)
}
```

Note: `worker_id` is removed from individual fields — it is no longer sent
in the protocol. The worker's identity is server-assigned and comes from
the token.

New method `WorkerConfig::resolve(&self) -> Result<ResolvedWorkerConfig>`:

1. Check `CBSD_WORKER_TOKEN` env var.
2. If set, base64url-decode → JSON-parse → `WorkerToken`. Log that env var
   takes precedence if config file `worker_token` is also set.
3. Else check `self.worker_token`.
4. If set, same decode path.
5. Else require `self.api_key` and `self.arch` (legacy mode — `worker_name`
   defaults to hostname).
6. Return a `ResolvedWorkerConfig` with all fields guaranteed present.

### `ResolvedWorkerConfig`

```rust
pub struct ResolvedWorkerConfig {
    pub server_url: String,
    pub api_key: String,
    pub worker_name: String,  // for local logging only, not sent over wire
    pub arch: Arch,
    // ... operational fields from WorkerConfig
}
```

### WS client updates

- `Hello` construction: remove `worker_id` field, set `protocol_version: 2`.
- `WorkerStopping` construction: remove `worker_id` field.
- The worker's structured log output uses `worker_name` from the resolved
  config for `tracing::info!` spans.

**Testable after:** `cargo build --workspace`. Worker can be started with
either a `worker_token` in the config, a `CBSD_WORKER_TOKEN` env var, or
the legacy `api_key` + `arch` fields. Worker connects with protocol v2.

---

## Commit 4: `server: update seed config to create registered workers`

**Files:**

- `cbsd-rs/cbsd-server/src/config.rs` (change `SeedWorkerKey` → `SeedWorker`)
- `cbsd-rs/cbsd-server/src/db/seed.rs` (create worker rows + print tokens)
- `cbsd-rs/config/server.yaml.example` (update seed section)
- `cbsd-rs/config/worker.yaml.example` (add `worker_token` field, update
  comments)

**Content:**

### Config change

```rust
#[derive(Debug, Deserialize)]
pub struct SeedWorker {
    pub name: String,
    pub arch: Arch,  // uses cbsd_proto::Arch — serde validates at parse time
}

#[derive(Debug, Default, Deserialize)]
pub struct SeedConfig {
    pub seed_admin: Option<String>,
    #[serde(default)]
    pub seed_workers: Vec<SeedWorker>,
}
```

Using `Arch` (the `cbsd-proto` enum with serde aliases) as the type means
a typo like `arch: armm64` fails at YAML parse time with a clear error —
no additional validation in `ServerConfig::validate()` needed. This is the
single validation source; the SQL CHECK constraint is the DB-level safety
net.

### Seed: add `workers:view` to `builder` role

Add `"workers:view"` to the `builder` role's cap list in the seed logic so
builders can see worker status. This is a seed-time-only change — existing
deployments with a `builder` role must be re-seeded (acceptable pre-release).

### Seed logic

In `run_first_startup_seed()`, for each `seed_workers` entry:

1. Generate worker UUID.
2. Generate random API key bytes and argon2 hash **before** opening the
   transaction (fixes a pre-existing issue where `generate_api_key_in_tx`
   calls `spawn_blocking` inside an open transaction, holding a pool
   connection during CPU-bound work).
3. Begin transaction (or extend the existing seed transaction).
4. Insert API key row via `insert_api_key_in_tx`. Get `api_key_id` via
   `result.last_insert_rowid()`.
5. Insert `workers` row.
6. Commit transaction.
7. After commit: build and print worker tokens to stdout (not structured
   logs).

The output changes from:

```
Worker API key for worker-x86-01: cbsk_a1b2c3...
```

to:

```
Worker token for worker-x86-01: eyJ3b3JrZXJfaWQ...
```

### Example config update

Update `cbsd-rs/config/server.yaml.example` to show the new `seed_workers`
format with `arch` field.

**Testable after:** Fresh server startup creates worker DB rows and prints
worker tokens. A worker configured with the printed token connects
successfully.

---

## Commit 5: `server: update GET /api/workers to merge DB and in-memory state`

**Files:**

- `cbsd-rs/cbsd-server/src/routes/workers.rs` (rewrite `list_workers`)
- `cbsd-rs/cbsd-server/src/queue/mod.rs` (update `WorkerInfo`, add helper)

**Content:**

### Merged worker listing

The current `GET /api/workers` only returns in-memory connected workers. After
this commit, it returns all registered workers with their current status.

Algorithm:

1. Query all rows from `workers` table.
2. Lock the `BuildQueue` and snapshot the in-memory worker map.
3. For each DB row:
   - Scan the in-memory map for a connection whose `registered_worker_id`
     matches the DB row's `id`.
   - If found: set `status` based on `WorkerState`:
     - `Connected` with no active build → `"connected"`
     - `Connected` with active build → `"building"`
     - `Stopping` → `"stopping"`
     - `Disconnected` → `"disconnected"`
   - If not found: set `status` = `"offline"`.
   - If the worker has an active build: include `current_build_id`.
4. Return the merged list.

### Updated `WorkerInfo`

```rust
#[derive(Debug, Serialize)]
pub struct WorkerInfo {
    pub worker_id: String,       // registered UUID
    pub name: String,
    pub arch: Arch,
    pub status: String,          // connected | building | stopping | disconnected | offline
    pub last_seen: Option<i64>,
    pub created_by: String,
    pub created_at: i64,
    pub current_build_id: Option<i64>,
}
```

The old `connection_id` field is removed from the public API (internal
implementation detail). If needed for debugging, it can be added as an
optional field later.

### Existing `GET /api/workers` route

The route path stays at `GET /api/workers`. The capability check stays at
`workers:view`.

**Testable after:** `GET /api/workers` returns registered workers with
accurate status even when no workers are connected.

---

## Dependency graph

```
Commit 1 → Commit 2 → Commit 3
                 ↓
             Commit 4
                 ↓
             Commit 5
```

- Commit 1 (DB + REST API) is the foundation.
- Commit 2 (WS binding) depends on Commit 1 (needs the worker lookup).
- Commit 3 (worker config) depends on Commit 2 (needs the server to accept
  token-based auth and protocol v2).
- Commit 4 (seed) depends on Commit 1 (needs worker insert + API key
  creation).
- Commit 5 (merged listing) depends on Commit 2 (needs `registered_worker_id`
  in `WorkerState`).

Commits 3, 4, and 5 are independent of each other and could be reordered.

---

## Backward compatibility

- **Worker config:** The legacy `api_key` + `arch` individual fields remain
  supported (the `worker_id` field is removed since it is no longer sent in
  the protocol). Existing worker configs need minor adjustment. The server
  will reject unregistered API keys at WS upgrade. Migration path: register
  the worker via the API, switch to the token.

- **Seed config:** `seed_worker_api_keys` is replaced by `seed_workers`.
  This is a breaking config change. Acceptable since cbsd-rs is pre-release.

- **`builds.worker_id` column:** Changes from display name to UUID. Existing
  build records (from before this change) will have the old display name.
  Queries joining `builds.worker_id` to `workers.id` will simply not match
  old records — acceptable since there are no production builds yet.

- **WebSocket protocol:** Breaking change — protocol version bumped from 1
  to 2. The `Hello` message drops `worker_id`. The `WorkerStopping` message
  drops `worker_id`. Workers running protocol v1 are rejected with a clear
  error. Acceptable since cbsd-rs is pre-release with no deployed workers.

- **Upgrade path:** Phase 7 requires a **fresh database**. There are no
  production cbsd-rs deployments. Existing development databases with workers
  seeded via the old `seed_worker_api_keys` config lack `workers` table rows
  and will be rejected at WS upgrade. Delete the dev DB and let the server
  re-seed with the new `seed_workers` config format.
