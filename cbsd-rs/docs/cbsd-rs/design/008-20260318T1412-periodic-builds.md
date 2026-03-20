# Periodic Builds

## Problem

cbsd-rs has no periodic/cron build support. Production deployments rely
on automated nightly builds and release candidates. The Python cbsd has
a full implementation with cron scheduling, tag interpolation, and
retry logic. cbsd-rs needs feature parity.

## Design

### Data model

A periodic build task is a build descriptor template paired with a cron
schedule and a tag format string. When the cron fires, the server
interpolates the tag, clones the descriptor with the new tag, and
submits the build internally.

```sql
CREATE TABLE IF NOT EXISTS periodic_tasks (
    id                  TEXT PRIMARY KEY,       -- UUID v4
    cron_expr           TEXT NOT NULL,          -- standard 5-field crontab
    tag_format          TEXT NOT NULL,          -- format string with {var} placeholders
    descriptor          TEXT NOT NULL,          -- JSON BuildDescriptor
    descriptor_version  INTEGER NOT NULL DEFAULT 1,
    priority            TEXT NOT NULL DEFAULT 'normal'
                        CHECK (priority IN ('high', 'normal', 'low')),
    summary             TEXT,                   -- optional description
    enabled             INTEGER NOT NULL DEFAULT 1,
    created_by          TEXT NOT NULL REFERENCES users(email),
    created_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Retry state (persisted across restarts)
    retry_count         INTEGER NOT NULL DEFAULT 0,
    retry_at            INTEGER,               -- next retry timestamp (NULL when not retrying)
    last_error          TEXT,
    -- Operational visibility
    last_triggered_at   INTEGER,
    last_build_id       INTEGER REFERENCES builds(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_periodic_enabled ON periodic_tasks(enabled);
```

Key fields:
- `id` — UUID v4, server-assigned at creation.
- `cron_expr` — standard 5-field crontab (minute hour day month weekday).
  All cron expressions are evaluated in **UTC**. Local-time scheduling
  is not supported.
- `tag_format` — format string with `{variable}` placeholders (see
  "Tag format interpolation" below). Applied to `descriptor.dst_image.tag`
  at trigger time.
- `descriptor` — JSON-serialized `BuildDescriptor`. This is the template
  — the tag and `signed_off_by` are overwritten at trigger time.
- `descriptor_version` — schema version for forward compatibility.
- `priority` — build priority for triggered builds (high/normal/low).
- `enabled` — 1 = active (scheduled), 0 = disabled (not scheduled).
- `created_by` — the user who created the task. References `users(email)`
  with no `ON DELETE` clause — users are deactivated, never deleted
  (same invariant as `workers.created_by`).
- `retry_count` — number of consecutive failed trigger attempts. Reset
  to 0 on successful trigger. Auto-disables at max retries (default 10).
- `retry_at` — Unix timestamp for the next retry attempt. NULL when not
  retrying. On startup, if `retry_at > now`, the scheduler sleeps until
  `retry_at`. If `retry_at <= now`, fires immediately (backoff elapsed
  during downtime). Cleared on success, disable, or re-enable.
- `last_error` — error message from the most recent failed trigger.
- `last_triggered_at` — Unix timestamp of the last successful trigger.
- `last_build_id` — build ID from the last successful trigger. FK to
  `builds(id) ON DELETE SET NULL`.

Additionally, the `builds` table gains a nullable FK for traceability:

```sql
ALTER TABLE builds ADD COLUMN periodic_task_id TEXT
    REFERENCES periodic_tasks(id) ON DELETE SET NULL;
```

This enables querying "all builds spawned by task X."

### Tag format interpolation

The `tag_format` string supports `{variable}` placeholders that are
substituted at trigger time. Two categories:

**Time-based variables** (evaluated at trigger time, UTC):

| Variable | Description | Example |
|---|---|---|
| `{Y}` | Year (4 digits) | `2026` |
| `{m}` | Month (01–12) | `03` |
| `{d}` | Day (01–31) | `18` |
| `{H}` | Hour (00–23) | `14` |
| `{M}` | Minute (00–59) | `30` |
| `{S}` | Second (00–59) | `20` |
| `{DT}` | ISO datetime | `20260318T143020` |

**Descriptor-based variables** (from the build descriptor template):

| Variable | Source | Example |
|---|---|---|
| `{version}` | `descriptor.version` | `19.2.3` |
| `{base_tag}` | `descriptor.dst_image.tag` | `v19.2.3` |
| `{channel}` | `descriptor.channel` | `ces` |
| `{user}` | `descriptor.signed_off_by.user` | `Alice` |
| `{arch}` | `descriptor.build.arch` | `x86_64` |
| `{distro}` | `descriptor.build.distro` | `rockylinux` |
| `{os_version}` | `descriptor.build.os_version` | `el9` |

**Example:** `tag_format = "{version}-nightly-{DT}"` with version `19.2.3`
triggered at 2026-03-18 02:00:00 UTC → `19.2.3-nightly-20260318T020000`.

Tag format is validated at task creation/update time: all `{...}`
placeholders must match a known variable name. Unknown variables are
rejected with 400. After interpolation, the result is validated against
OCI tag constraints (max 128 chars, `[a-zA-Z0-9_][a-zA-Z0-9_.-]*`).

A constant `tag_format` (e.g., `"latest"`) is valid but produces the
same tag every trigger. Recommend including at least one time-varying
variable for uniqueness.

Note: `{S}` (seconds) is typically `00` for cron-triggered builds
since cron is minute-granular. Under retry, `{S}` may be non-zero
(the retry fires at an exact `retry_at` timestamp). Included for
compatibility with the Python implementation.

### Scheduler

A single tokio task runs the scheduler. It maintains a priority queue
(sorted by next-fire-time) of all enabled periodic tasks. The scheduler
sleeps until the earliest next-fire-time, triggers that task, computes
its next fire time, re-inserts it into the queue, and sleeps again.

```
Scheduler loop:
  1. Load all enabled tasks from DB.
  2. Compute next-fire-time for each (cron or retry_at).
  3. Sort by next-fire-time. If multiple share the same time,
     order by task id (deterministic).
  4. Sleep until the earliest one (or until notified).
  5. Wake up, trigger that task (submit build).
  6. On success: reset retry_count, clear retry_at, update
     last_triggered_at and last_build_id, compute next cron fire.
  7. On transient failure: increment retry_count, compute
     retry_at with backoff, persist both to DB.
  8. On non-transient failure or max retries: disable task,
     set last_error, clear retry_at.
  9. Go to step 2.
```

**State changes (new task, update, delete, enable, disable) interrupt
the scheduler.** The scheduler listens on a `tokio::sync::Notify` using
`notify_one()`. When any mutation occurs:

1. The REST handler modifies the DB.
2. The REST handler calls `scheduler_notify.notify_one()`.
3. The scheduler wakes, reloads from DB (step 2), and re-enters the
   sleep loop. The scheduler always re-checks state before sleeping
   to handle notifications that arrived during processing.

Because the reload at step 2 is a full scan of `periodic_tasks`, a
single wakeup after a burst of mutations converges to correct state.
Multiple concurrent `notify_one()` calls may coalesce into a single
reload, which is acceptable.

**Startup:** After `run_startup_recovery` and before accepting HTTP
connections (same position as `sweep_handle` and `gc_handle`), the
scheduler loads all enabled tasks, logs a summary (count + next fire
times), and enters the loop. Missed fires during downtime are skipped
— a warning is logged for each task whose computed next fire was in the
past.

**Shutdown:** The scheduler handle follows the existing
`Arc<Mutex<Option<JoinHandle<()>>>>` pattern. On SIGTERM, the handle
is aborted alongside `sweep_handle` and `gc_handle`. No special
cleanup needed — the scheduler is stateless beyond the DB.

**Retry vs cron sleep:** Each task has either a cron-computed next fire
time or a retry-computed `retry_at` timestamp (whichever is applicable).
The scheduler's priority queue sorts all tasks by their effective next
fire time regardless of type. A task in retry has `retry_at` as its
next fire time; a healthy task uses its cron expression.

### Build submission at trigger time

When a periodic task fires:

1. Check that the `created_by` user is still active (`users.active`).
   If deactivated: disable the task, log a warning, skip.
2. Look up the `created_by` user's current `name` and `email` from the
   `users` table.
3. Clone the `BuildDescriptor` from the task.
4. Set `descriptor.signed_off_by.user` and `.email` to the looked-up
   values from step 2 (ensures `{user}` resolves correctly and audit
   attribution is consistent).
5. Interpolate `tag_format` with current UTC time + descriptor variables.
6. Validate the interpolated tag against OCI constraints.
7. Set `descriptor.dst_image.tag` to the interpolated value.
8. Submit the build internally — call the same build insertion +
   queue-enqueue logic used by `POST /api/builds/`, but **bypass
   permission checks**. Set:
   - `builds.periodic_task_id` to the task ID.
   - `builds.user_email` to `periodic_tasks.created_by`.
   - `builds.priority` from `periodic_tasks.priority`.
9. Update `last_triggered_at`, `last_build_id`, reset `retry_count`,
   clear `retry_at`.

**Security note on permission bypass:** Periodic tasks represent a
permanent delegation of the creator's build permissions at creation
time. Revoking a user's `builds:create` role or narrowing their scopes
does NOT stop their periodic tasks — an admin must manually disable
them. The only automatic gate is the `users.active` check: deactivating
a user disables all their periodic tasks at next trigger.

At **creation time**, the server validates:
- `periodic:create` capability on the requesting user.
- `builds:create` capability on the requesting user.
- Full scope validation (channel, registry, repository) against the
  descriptor — the same checks as `POST /api/builds/`.
- Cron expression validity.
- Tag format validity (known placeholders).
- Descriptor validity (known components, valid arch, etc.).

This is the only permission gate. Once created, the task runs with
only the `users.active` check.

If no workers are connected when a periodic build triggers, the build
queues normally and dispatches when a worker reconnects (same as manual
builds).

No deduplication is performed: if a build from the previous trigger is
still queued/active when the next cron fires, a second build is
submitted. This is documented as known behavior.

### Retry behavior

If the build submission fails at trigger time:

- **Transient errors** (database contention, internal error): increment
  `retry_count`, compute `retry_at` as `now + backoff_secs` (30s
  initial, 1.5x multiplier, 10-minute ceiling), persist both to DB.
  The scheduler sleeps until `retry_at` and retries.
- **Max retries exhausted** (default 10): disable the task, set
  `last_error`, clear `retry_at`. Log an error.
- **Non-transient errors** (descriptor validation failure, unknown
  component): disable the task immediately, set `last_error`, clear
  `retry_at`. Log an error.
- **User deactivated** (from step 1): disable the task, clear
  `retry_at`. Log a warning.

On successful trigger: reset `retry_count` to 0, clear `retry_at`,
clear `last_error`.

Retry state (`retry_count`, `retry_at`, `last_error`) is persisted to
the DB, surviving server restarts. At startup, if a task has
`retry_at > now`, the scheduler uses `retry_at` as the next fire time.
If `retry_at <= now`, the scheduler fires immediately (the backoff
elapsed during downtime).

A disabled task can be re-enabled via `PUT /api/periodic/{id}/enable`.
Re-enabling resets `retry_count` to 0 and clears `retry_at`.

### REST API

#### `POST /api/periodic` — Create periodic task

**Requires:** `periodic:create` + `builds:create` (with full scope
validation against the descriptor).

**Request body:**
```json
{
  "cron_expr": "0 2 * * *",
  "tag_format": "{version}-nightly-{DT}",
  "descriptor": { ... BuildDescriptor ... },
  "priority": "normal",
  "summary": "Nightly CES build"
}
```

**Response (201):** Full resource shape (same as list response, avoids
an immediate follow-up GET):
```json
{
  "id": "550e8400-...",
  "cron_expr": "0 2 * * *",
  "tag_format": "{version}-nightly-{DT}",
  "descriptor": { ... },
  "priority": "normal",
  "summary": "Nightly CES build",
  "enabled": true,
  "created_by": "admin@example.com",
  "created_at": 1710720000,
  "updated_at": 1710720000,
  "retry_count": 0,
  "last_error": null,
  "last_triggered_at": null,
  "last_build_id": null,
  "next_run": 1710727200
}
```

**Validation:**
- `cron_expr` — parsed by the `croner` crate (standard 5-field crontab).
  Reject invalid expressions.
- `tag_format` — all `{...}` placeholders must be known variables.
- `descriptor` — same validation as `POST /api/builds/` (known
  components, valid arch, etc.) but do NOT submit a build.
- `priority` — optional, defaults to `"normal"`.

#### `GET /api/periodic` — List all periodic tasks

**Requires:** `periodic:view`.

**Response:**
```json
[
  {
    "id": "550e8400-...",
    "cron_expr": "0 2 * * *",
    "tag_format": "{version}-nightly-{DT}",
    "descriptor": { ... },
    "priority": "normal",
    "summary": "Nightly CES build",
    "enabled": true,
    "created_by": "admin@example.com",
    "created_at": 1710720000,
    "updated_at": 1710720000,
    "retry_count": 0,
    "last_error": null,
    "last_triggered_at": 1710640000,
    "last_build_id": 42,
    "next_run": 1710727200
  }
]
```

`next_run` is `null` when `enabled` is false.

#### `GET /api/periodic/{id}` — Get a specific periodic task

**Requires:** `periodic:view`.

Same response shape as a single list element. 404 if not found.

#### `PUT /api/periodic/{id}` — Update a periodic task

**Requires:** `periodic:manage`. If the request includes a `descriptor`
field, additionally requires `builds:create` with full scope validation
against the updated descriptor (same checks as POST creation). This
prevents a user with `periodic:manage` but limited build scopes from
redirecting a task to a channel they don't have access to.

**Request body** (all fields optional — at least one must be provided):
```json
{
  "cron_expr": "0 3 * * 1",
  "tag_format": "{version}-weekly-{DT}",
  "descriptor": { ... },
  "priority": "high",
  "summary": "Weekly CES build"
}
```

Same validation as creation for any provided field. `updated_at` is set
explicitly to `unixepoch()` in the UPDATE query. PUT does not change
`enabled` state — use the separate enable/disable endpoints.

For nullable fields: `"summary": null` clears the field. Omitting
`summary` from the request leaves it unchanged.

After update, the scheduler is notified to recompute next-fire-times.
If the task is enabled, the new schedule takes effect immediately.

Returns 200 on success (full resource shape). Returns 400 if body is
empty. Returns 404 if task not found.

#### `DELETE /api/periodic/{id}` — Delete a periodic task

**Requires:** `periodic:manage`.

Removes the task from the DB entirely. If the task is enabled, the
scheduler is notified. Returns 200 with confirmation. 404 if not found.

#### `PUT /api/periodic/{id}/enable` — Enable a disabled task

**Requires:** `periodic:manage`.

Sets `enabled = 1`, resets `retry_count` to 0, clears `retry_at` and
`last_error`. Notifies the scheduler to add the task to the queue.
Returns 200.

#### `PUT /api/periodic/{id}/disable` — Disable an active task

**Requires:** `periodic:manage`.

Sets `enabled = 0`, clears `retry_at`. Notifies the scheduler to
remove the task from the queue. If a retry is in progress, it is
cancelled (the scheduler will skip the task on its next wake).
Returns 200.

### Capabilities

| Capability | Description |
|---|---|
| `periodic:create` | Create periodic tasks |
| `periodic:view` | List and inspect periodic tasks |
| `periodic:manage` | Update, delete, enable, disable periodic tasks |

All three are added to `KNOWN_CAPS` in `permissions.rs`. The `admin`
role's wildcard `*` already covers them. Only admins get periodic build
capabilities at this point.

`periodic:create` and `periodic:manage` are independent capabilities —
`periodic:manage` is NOT a superset of `periodic:create`. Creating a
task requires `periodic:create` + `builds:create` (with scopes).
Managing existing tasks requires `periodic:manage` (and `builds:create`
with scopes if updating the descriptor).

### Cron expression handling

The `croner` crate v3.0 (Rust) natively supports standard 5-field
crontab. Added to `cbsd-server/Cargo.toml` as `croner = "3"`.

```
┌───────────── minute (0–59)
│ ┌───────────── hour (0–23)
│ │ ┌───────────── day of month (1–31)
│ │ │ ┌───────────── month (1–12)
│ │ │ │ ┌───────────── day of week (0–7, 0 and 7 are Sunday)
│ │ │ │ │
* * * * *
```

Supports `*`, ranges (`1-5`), lists (`1,3,5`), steps (`*/15`).
Shorthands `@daily`, `@weekly`, `@monthly`, `@yearly` are supported.

**Parsing API:**
```rust
use std::str::FromStr;
use croner::{Cron, Direction};

let cron = Cron::from_str("0 2 * * *")?;
let next = cron.iter_from(chrono::Utc::now(), Direction::Forward).next();
```

Leap-year cron expressions (`0 0 29 2 *`) are handled — the iterator
skips to the next valid date.

At startup, if a previously stored cron expression fails to parse
(e.g., after a crate update changes validation rules), the scheduler
logs a warning and skips that task (does not abort). The task remains
in the DB and can be updated via the API.

### Migration

New migration `003_periodic_tasks.sql`:
- Creates the `periodic_tasks` table.
- Adds `periodic_task_id` column to `builds` table.

### Server state

`AppState` gains:
- `scheduler_notify: Arc<tokio::sync::Notify>` — wakeup mechanism.
- `scheduler_handle: Arc<Mutex<Option<JoinHandle<()>>>>` — task handle,
  same pattern as `sweep_handle` and `gc_handle`.

### Shutdown behavior

On server shutdown (SIGTERM), the scheduler handle is aborted alongside
`sweep_handle` and `gc_handle`. No special cleanup needed — the
scheduler is stateless beyond the DB.

On drain shutdown (SIGQUIT), same behavior — periodic tasks are
scheduling metadata, not in-flight builds.
