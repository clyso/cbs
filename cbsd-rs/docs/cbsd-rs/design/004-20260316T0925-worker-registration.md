# Worker Registration Model

## Problem

The current cbsd-rs worker identity model has three weaknesses:

1. **No persistent worker identity.** Workers are tracked only in memory, keyed
   by a server-assigned connection UUID that changes on every reconnect. The
   `worker_id` in the `Hello` message is a self-reported display label with no
   uniqueness constraint. Two workers can claim the same `worker_id`.

2. **API keys are not bound to workers.** A single API key can authenticate
   multiple workers, and a single worker's key cannot be revoked without
   affecting others that share the same key. There is no server-side record
   linking a specific key to a specific worker.

3. **Bootstrap friction.** Deploying a worker requires manually capturing an
   API key from server logs, editing the worker config file, and restarting.
   This makes compose-style and automated deployments difficult.

## Goals

- Server-assigned, persistent worker identity (UUID) stored in the database.
- One-to-one binding between a registered worker and its API key.
- Single REST call to register a worker, returning everything the worker needs
  as a base64-encoded JSON blob (the "worker token").
- Worker accepts the token via config file or environment variable — no manual
  assembly of multiple fields.
- Individual worker revocation without affecting other workers.
- `GET /api/workers` returns both connected and registered-but-offline workers.

## Design

### Worker lifecycle

```
            POST /api/admin/workers          Worker connects via WS
                     │                                │
                     ▼                                ▼
            ┌─────────────┐                   ┌──────────────┐
            │  Registered  │ ── worker starts ─►│  Connected   │
            │  (DB row)    │                   │  (DB + mem)  │
            │  status=idle │                   │  status=...  │
            └─────────────┘                   └──────────────┘
                     │                                │
                     ▼                                │
          DELETE /api/admin/workers/{id}              │
                     │                                │
                     ▼                    WS disconnect │
            ┌─────────────┐                   ┌──────────────┐
            │   Deleted    │                   │ Disconnected │
            │  (row gone,  │                   │  (grace per.)│
            │   key revoked)│                   └──────────────┘
            └─────────────┘
```

### Database: `workers` table

New table in migration `002_worker_registration.sql`:

```sql
CREATE TABLE IF NOT EXISTS workers (
    id          TEXT PRIMARY KEY,       -- server-assigned UUID v4
    name        TEXT NOT NULL UNIQUE,   -- human-readable label
    arch        TEXT NOT NULL CHECK (arch IN ('x86_64', 'aarch64')),
    api_key_id  INTEGER NOT NULL UNIQUE REFERENCES api_keys(id) ON DELETE CASCADE,
    created_by  TEXT NOT NULL REFERENCES users(email),
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen   INTEGER                -- updated on WS connect/heartbeat
);
```

Key decisions:

- **`id` is a UUID v4 string**, not autoincrement. UUIDs avoid leaking fleet
  size and are safe to expose in tokens.
- **`name` is UNIQUE** — prevents accidental duplicate names.
- **`api_key_id` is UNIQUE** and cascades on delete — one key per worker.
  Deleting the worker row triggers API key deletion via the cascade
  relationship on `api_key_id`. However, the actual cleanup happens in the
  application layer: the `DELETE /api/admin/workers/{id}` handler revokes the
  API key first (setting `revoked=1`, purging the LRU cache), then deletes
  the worker row. The FK cascade is a safety net, not the primary mechanism.
- **`last_seen`** is updated on WS handshake and on `build_finished`. This
  gives operators a reliable proof-of-life signal for fleet monitoring.
- **`created_by`** tracks who registered the worker, for audit purposes.
  References `users(email)` with no `ON DELETE` clause — users are deactivated,
  never deleted. This is an invariant of the user model.

### Worker token (the blob the worker receives)

When a worker is registered, the server returns a base64url-encoded JSON
object containing everything the worker needs to connect:

```json
{
  "worker_id": "550e8400-e29b-41d4-a716-446655440000",
  "worker_name": "build-host-01",
  "api_key": "cbsk_a1b2c3d4e5f6...",
  "arch": "x86_64"
}
```

This is the **worker token** — a JSON object encoded as base64url (RFC 4648
§5, no padding). It is shown exactly once (at registration time) and cannot
be recovered later because the API key plaintext is never stored.

The worker accepts this token via:

- **Config file:** `worker_token: "eyJ3b3JrZXJfaWQ..."` in `worker.yaml`
- **Environment variable:** `CBSD_WORKER_TOKEN=eyJ3b3JrZXJfaWQ...`

When a worker token is present, it overrides `api_key` and `arch` fields in
the config (if present). The `server_url` is NOT included in the token — it
is deployment-specific and must be configured separately.

### REST API

#### `POST /api/admin/workers` — Register a new worker

**Requires:** `workers:manage` capability.

**Request body:**

```json
{
  "name": "build-host-01",
  "arch": "x86_64"
}
```

**Response (201 Created):**

```json
{
  "worker_id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "build-host-01",
  "arch": "x86_64",
  "worker_token": "eyJ3b3JrZXJfaWQ..."
}
```

**Server-side actions (atomic transaction):**

1. Generate UUID v4 for the worker.
2. Hash the API key with argon2 **before** opening the transaction (argon2 is
   CPU-bound and must not hold a pool connection).
3. Begin transaction.
4. Insert API key row (name: `worker:<name>`, owner: requesting user).
   Use `last_insert_rowid()` to get the `api_key_id` — no second query.
5. Insert into `workers` table with the `api_key_id`.
6. Commit transaction.
7. Return the worker token (base64url of JSON with id, name, key, arch).

The API key name is prefixed with `worker:` to distinguish worker keys from
user-created keys in listings. Worker-prefixed keys are filtered from the
self-service `GET /api/auth/api-keys` listing to prevent accidental deletion.

#### `GET /api/workers` — List all registered workers

**Requires:** `workers:view` capability (same path as the existing endpoint,
now updated to merge DB and in-memory state).

**Response:**

```json
[
  {
    "worker_id": "550e8400-...",
    "name": "build-host-01",
    "arch": "x86_64",
    "status": "connected",
    "last_seen": 1710576000,
    "created_by": "admin@clyso.com",
    "created_at": 1710547200,
    "current_build_id": 42
  },
  {
    "worker_id": "660f9500-...",
    "name": "arm-builder-02",
    "arch": "aarch64",
    "status": "offline",
    "last_seen": 1710460000,
    "created_by": "admin@clyso.com",
    "created_at": 1710460000,
    "current_build_id": null
  }
]
```

The `status` field is derived by joining the DB `workers` table with the
in-memory `BuildQueue.workers` map:

- `"connected"` — worker has an active WS connection, idle.
- `"building"` — worker is connected and has an active build.
- `"stopping"` — worker announced graceful shutdown.
- `"disconnected"` — worker WS dropped, within grace period.
- `"offline"` — no active WS connection and not in grace period.

#### `DELETE /api/admin/workers/{id}` — Deregister a worker

**Requires:** `workers:manage` capability.

**Response (200):**

```json
{
  "detail": "worker 'build-host-01' deregistered",
  "api_key_revoked": true
}
```

**Server-side actions:**

1. Look up the worker row to find its `api_key_id` and `name`.
2. Revoke the API key by ID (`revoke_api_key_by_id(pool, api_key_id)` — does
   not filter by `owner_email`, so any admin can deregister any worker).
3. Purge the API key from the LRU cache.
4. Delete the worker row.
5. If the worker is currently connected, force-disconnect it:
   a. Lock `BuildQueue` → scan for connection whose `registered_worker_id`
      matches → extract `connection_id` → remove entry from map → **release
      lock.** (The queue mutex must be released before step 5c because
      `handle_worker_dead` re-acquires it — `tokio::sync::Mutex` is not
      reentrant.)
   b. Remove `connection_id` from `state.worker_senders`. This drops the
      `UnboundedSender`, closing `outbound_rx`, which terminates the forward
      task and ultimately triggers `cleanup_worker`. Since the
      `BuildQueue.workers` entry was already removed, `cleanup_worker` finds
      nothing and bails.
   c. Call `handle_worker_dead(state, connection_id)` to re-queue any
      in-flight build (uses `ab.priority`, not hardcoded `Priority::Normal`).

#### `POST /api/admin/workers/{id}/regenerate-token` — Rotate API key

**Requires:** `workers:manage` capability.

Revokes the old API key, generates a new one, returns a fresh worker token.
Useful for key rotation without deleting and re-creating the worker.

**Server-side actions (atomic transaction):**

1. Hash the new API key with argon2 **before** opening the transaction.
2. Begin transaction.
3. Insert new API key row.
4. Update `workers.api_key_id` to the new key's row ID.
5. Revoke the old API key (set `revoked=1`).
6. Commit transaction.
7. Purge old API key from LRU cache.
8. If the worker is currently connected: force-disconnect using the same
   sequence as deregistration (lock queue → extract connection_id → remove
   from map → release lock → remove from worker_senders → call
   `handle_worker_dead`). Re-queues any in-flight build.

The transaction ordering (insert new → update FK → revoke old) ensures that
a crash at any point leaves the worker with at least one valid key. If the
crash is before commit, the old key is still valid. If after commit, the new
key is active.

**Response (200):**

```json
{
  "worker_id": "550e8400-...",
  "name": "build-host-01",
  "arch": "x86_64",
  "worker_token": "eyJ3b3JrZXJfaWQ..."
}
```

The operator must update the worker's config/env with the new token and
restart.

### WebSocket handshake changes

Currently the WS `Hello` message includes a self-reported `worker_id` string
and an `arch` field. After this change:

1. **Auth at WS upgrade** remains unchanged — the API key is verified from the
   `Authorization: Bearer cbsk_...` header.
2. After successful API key verification, the server looks up the `workers`
   table row where `api_key_id` matches the verified key's DB row ID.
   - If no worker row exists: reject with `"API key is not bound to a
     registered worker"`. This prevents unregistered keys from establishing
     WS connections.
   - If found: the server now knows the worker's UUID, name, and arch from
     the DB. This identity is passed to `handle_connection`.
3. The `worker_id` field is **removed from the `Hello` message**. The server
   uses the DB-registered identity (looked up at step 2), not a self-reported
   value. This is a protocol-breaking change (protocol version bump to 2).
4. The `Hello.arch` field is **kept and validated** against `workers.arch`. A
   mismatch is an error — the server disconnects with a clear error message
   (e.g., `"arch mismatch: worker registered as x86_64 but reported
   aarch64"`). This catches operators who copy a worker token between machines
   with different architectures.
5. The server updates `workers.last_seen` on successful handshake and on
   `build_finished`.

### Protocol version bump

The `Hello` message changes shape (drops `worker_id`), which requires a
protocol version bump from 1 to 2. The `Welcome` message already includes
`protocol_version` and `min_version`/`max_version` in the `Error` message.

The server accepts only protocol version 2. Workers running the old protocol
(version 1) receive an `Error` message with `min_version: 2, max_version: 2`
and are disconnected. Since cbsd-rs is pre-release, there are no deployed v1
workers to worry about.

### `Hello` and `WorkerStopping` message changes

**Before (protocol v1):**

```rust
Hello {
    protocol_version: u32,
    worker_id: String,       // REMOVED in v2
    arch: Arch,
    cores_total: u32,
    ram_total_mb: u64,
}

WorkerStopping {
    worker_id: String,       // REMOVED in v2
    reason: String,
}
```

**After (protocol v2):**

```rust
Hello {
    protocol_version: u32,
    arch: Arch,              // validated against DB
    cores_total: u32,
    ram_total_mb: u64,
}

WorkerStopping {
    reason: String,
}
```

The `worker_id` in `WorkerStopping` was only used for logging. The server
already knows the worker's identity from the connection map.

### Worker config changes (`WorkerConfig`)

The `WorkerConfig` struct gains a `worker_token` field. Resolution order:

1. `CBSD_WORKER_TOKEN` env var (highest priority)
2. `worker_token` field in YAML config
3. Individual fields (`api_key`, `arch`) in YAML config (legacy — `worker_id`
   is no longer sent in the protocol)

When a token is present, the individual fields are ignored (if also set, a
warning is logged). When no token is present, the individual fields are
required (current behavior minus `worker_id`, which is no longer needed).

The `worker_name` from the token is used only for the worker's own structured
log output (so the operator can identify which worker a log line belongs to).
It is never sent over the wire.

### Seed config changes

The `seed_worker_api_keys` config section is replaced by `seed_workers`:

```yaml
seed:
  seed_admin: "admin@example.com"
  seed_workers:
    - name: worker-x86-01
      arch: x86_64
    # - name: worker-arm-01
    #   arch: aarch64
```

`SeedWorker.arch` is typed as `Arch` (the `cbsd-proto` enum), so invalid
values fail at YAML parse time — no separate validation step needed.

Each entry creates a `workers` row + API key at first startup, and the
plaintext worker tokens are printed to stdout. This maintains the existing
bootstrapping workflow but now produces tokens instead of bare API keys.

The old `seed_worker_api_keys` format is no longer accepted (breaking change
confined to the config file, acceptable for a pre-release project).

### `builds.worker_id` column

Currently stores the self-reported display label. After this change, it stores
the **registered worker UUID** (matching `workers.id`). This enables:

- Joining builds to workers for fleet analytics.
- Querying "all builds executed by worker X".
- Surviving worker renames (the UUID is stable).

### Worker name validation

Worker names must match `[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}` — alphanumeric,
hyphens, underscores, 1–64 characters, starting with alphanumeric. Validated
in the application layer (not a SQL CHECK constraint). The UNIQUE constraint
on `workers.name` prevents duplicates.

### Capability additions

| Capability | Description |
|---|---|
| `workers:manage` | Register, deregister, rotate worker tokens |
| `workers:view` | List workers and their status (existing) |

The `admin` builtin role gets `*` (already covers everything). The `builder`
role gets `workers:view` (so builders can see worker status) but not
`workers:manage` — worker registration is an admin operation.

`workers:manage` must be added to `KNOWN_CAPS` in `permissions.rs` so that
custom roles can include it.

### Worker API key isolation

Worker API keys (named `worker:<name>`) are filtered from the self-service
`GET /api/auth/api-keys` listing. This prevents users from accidentally
revoking a worker key via `DELETE /api/auth/api-keys/{prefix}`. Worker keys
are managed exclusively through the `/api/admin/workers` endpoints.

### Lost-token recovery

If a worker token is lost (operator can no longer access the plaintext), the
recovery workflow is:

1. `POST /api/admin/workers/{id}/regenerate-token` — generates a new token.
2. Update the worker's config/env with the new token.
3. Restart the worker.

Alternatively: deregister the worker (`DELETE /api/admin/workers/{id}`) and
re-register (`POST /api/admin/workers`) with the same name and arch.

### Upgrade path

Phase 7 requires a **fresh database**. There are no production cbsd-rs
deployments. Existing development databases with workers seeded via the old
`seed_worker_api_keys` config do not have `workers` table rows and will be
rejected at WS upgrade after this change. The simplest path is to delete the
development DB and let the server re-seed on startup with the new
`seed_workers` config format.

### Reconnection identity

When a worker reconnects after a disconnect, the server matches the
reconnecting worker to its previous connection by looking up the `workers`
table via the API key. Since `api_key_id` is unique per worker, the server
deterministically identifies which worker is reconnecting, regardless of
connection UUID. This replaces the current heuristic of matching by
self-reported `worker_id` string.

The reconnection flow:

1. Worker establishes new WS connection, authenticates with its API key.
2. Server looks up `workers` row by `api_key_id` → gets worker UUID.
3. In `handle_connection`, after receiving a valid `Hello` and **before
   registering the new connection**, the server scans `BuildQueue.workers`
   under the queue lock for any existing entry whose
   `registered_worker_id` matches:
   a. If found and `Disconnected`: under the queue lock, migrate active
      build `connection_id` references from old to new, remove the old
      entry, register the new connection. After releasing the queue lock,
      remove old `connection_id` from `worker_senders`. The in-flight
      build is preserved.
   b. If found and `Connected` (stale double-connect): treat identically
      to force-disconnect. Under the queue lock: extract old
      `connection_id`, remove old entry, register new. After releasing:
      remove old `connection_id` from `worker_senders`, call
      `handle_worker_dead(state, old_connection_id)` to re-queue any
      active build. Log a warning.
   c. If not found: register as a fresh connection.
4. Queue map mutations (migrate, remove old, register new) are atomic under
   one queue lock. `worker_senders` cleanup and `handle_worker_dead` happen
   **after** releasing the queue lock — `worker_senders` is a separate mutex
   and must not be nested inside the queue lock (lock inversion:
   `cleanup_worker` acquires `worker_senders` first, then queue). The grace-
   period monitor cannot fire during the migration because it also needs the
   queue lock.

This is strictly more reliable than the current approach because the identity
comes from a cryptographic proof (the API key), not a self-reported string.

## Decisions (resolved)

1. **`workers:manage` is admin-only.** Builders cannot self-serve worker
   registration. The `builder` role does not get `workers:manage`.

2. **Deregistration force-disconnects the worker and re-queues its build.**
   Once a worker is deregistered, we no longer trust it — the in-flight build
   is re-queued via the existing dead-worker resolution mechanism.

3. **`Hello` message drops `worker_id`, keeps `arch`.** The `worker_id` field
   is removed from the `Hello` message — the server already knows the worker's
   identity from the API key lookup at WS upgrade. The `arch` field is kept
   and validated against the registered value: a mismatch is an error that
   disconnects the worker with a log message. This catches admins copying
   worker tokens between machines with different architectures.
