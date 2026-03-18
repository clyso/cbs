# Phase 10: Periodic Builds

**Design document:** `_docs/cbsd-rs/design/2026-03-18-periodic-builds.md`

## Progress

| # | Commit | ~LOC | Status |
|---|--------|------|--------|
| 1 | `cbsd-rs/server: add periodic build scheduling` | ~1100 | Done |

**Total:** ~1100 LOC, 1 commit.

---

## Why one commit

The periodic builds feature has no useful intermediate state. The REST
API creates tasks that only matter if the scheduler fires them. The
scheduler fires tasks that only exist if the API created them. Tag
interpolation is only called by the trigger. The trigger is only called
by the scheduler. Splitting into multiple commits produces dead code in
every intermediate state.

~1100 LOC exceeds the 400–800 guideline, but the changes are mechanical:
CRUD endpoints follow established patterns, the scheduler is a
straightforward tokio loop, tag interpolation is string substitution,
and the DB layer is the same query!() pattern used throughout.

---

## Commit 1: `cbsd-rs/server: add periodic build scheduling`

**Files:**

- `cbsd-rs/migrations/003_periodic_tasks.sql` (new)
- `cbsd-rs/cbsd-server/Cargo.toml` (add `croner = "3"`)
- `cbsd-rs/cbsd-server/src/db/periodic.rs` (new)
- `cbsd-rs/cbsd-server/src/db/mod.rs` (add `pub mod periodic`)
- `cbsd-rs/cbsd-server/src/scheduler/mod.rs` (new)
- `cbsd-rs/cbsd-server/src/scheduler/trigger.rs` (new)
- `cbsd-rs/cbsd-server/src/scheduler/tag_format.rs` (new)
- `cbsd-rs/cbsd-server/src/routes/periodic.rs` (new)
- `cbsd-rs/cbsd-server/src/routes/mod.rs` (add `pub mod periodic`)
- `cbsd-rs/cbsd-server/src/routes/builds.rs` (extract
  `insert_build_internal()`)
- `cbsd-rs/cbsd-server/src/routes/permissions.rs` (add 3 caps to
  `KNOWN_CAPS`)
- `cbsd-rs/cbsd-server/src/app.rs` (add scheduler_notify, handle,
  nest route)
- `cbsd-rs/cbsd-server/src/main.rs` (spawn scheduler, shutdown)
- `cbsd-rs/.sqlx/` (regenerated)

---

### Migration `003_periodic_tasks.sql`

```sql
CREATE TABLE IF NOT EXISTS periodic_tasks (
    id                  TEXT PRIMARY KEY,
    cron_expr           TEXT NOT NULL,
    tag_format          TEXT NOT NULL,
    descriptor          TEXT NOT NULL,
    descriptor_version  INTEGER NOT NULL DEFAULT 1,
    priority            TEXT NOT NULL DEFAULT 'normal'
                        CHECK (priority IN ('high', 'normal', 'low')),
    summary             TEXT,
    enabled             INTEGER NOT NULL DEFAULT 1,
    created_by          TEXT NOT NULL REFERENCES users(email),
    created_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at          INTEGER NOT NULL DEFAULT (unixepoch()),
    retry_count         INTEGER NOT NULL DEFAULT 0,
    retry_at            INTEGER,
    last_error          TEXT,
    last_triggered_at   INTEGER,
    last_build_id       INTEGER REFERENCES builds(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_periodic_enabled
    ON periodic_tasks(enabled);

ALTER TABLE builds ADD COLUMN periodic_task_id TEXT
    REFERENCES periodic_tasks(id) ON DELETE SET NULL;
```

---

### DB module `db/periodic.rs`

Functions (all use `query!()`):

- `insert_task(pool, task) -> Result<()>`
- `get_task(pool, id) -> Result<Option<PeriodicTaskRow>>`
- `list_tasks(pool) -> Result<Vec<PeriodicTaskRow>>`
- `list_enabled_tasks(pool) -> Result<Vec<PeriodicTaskRow>>`
- `update_task(pool, id, fields...) -> Result<bool>`
- `delete_task(pool, id) -> Result<bool>`
- `set_enabled(pool, id, enabled, clear_retry) -> Result<bool>`
- `update_trigger_success(pool, id, build_id) -> Result<()>`
- `update_retry(pool, id, retry_count, retry_at, last_error) -> Result<()>`
- `disable_with_error(pool, id, last_error) -> Result<()>`

---

### Tag format `scheduler/tag_format.rs`

- `validate_tag_format(format: &str) -> Result<(), Vec<String>>`
  Checks all `{...}` placeholders are known. Returns list of unknown
  variables on error.
- `interpolate_tag(format: &str, descriptor: &BuildDescriptor, now: DateTime<Utc>) -> String`
  Substitutes all 14 variables.
- `validate_oci_tag(tag: &str) -> Result<(), String>`
  Length <= 128, charset `[a-zA-Z0-9_][a-zA-Z0-9_.-]*`.

Unit tests for all three.

---

### Scheduler `scheduler/mod.rs`

Loop:

1. `list_enabled_tasks()` from DB.
2. For each: compute effective fire time.
   - `retry_at` set and future → use it.
   - `retry_at` set and past → fire immediately.
   - Otherwise → `Cron::from_str(cron_expr)` + `iter_from(now, Forward)`.
   - Parse failure → log warning, skip.
   - Next occurrence in past (missed fire) → log warning, skip to next
     future occurrence.
3. Sort by fire time, tiebreak by id.
4. `tokio::select!` sleep-until-earliest vs `scheduler_notify`.
5. On sleep wake: re-fetch task from DB (not cached). If gone or
   disabled, skip. Otherwise call `trigger_periodic_build()`.
6. On notify wake: go to step 1 (full reload).

Startup: after `run_startup_recovery`, spawn scheduler, log summary
(enabled task count + next fire times).

Shutdown: abort handle alongside `sweep_handle` and `gc_handle`.

---

### Build trigger `scheduler/trigger.rs`

`trigger_periodic_build(state, task) -> Result<i64, TriggerError>`:

1. Check `users.active` for `created_by`. Inactive → `UserDeactivated`.
2. Look up user name/email from `users` table.
3. Clone descriptor, set `signed_off_by` from user.
4. Interpolate tag. Validate OCI tag.
5. Call `insert_build_internal()` with `periodic_task_id`,
   `user_email = created_by`, `priority = task.priority`.
6. Return build ID.

Error types:
- `UserDeactivated` → disable task.
- `Transient(String)` → retry (30s initial, 1.5x, 10min ceiling,
  max 10 attempts).
- `Fatal(String)` → disable task.

---

### Extract `insert_build_internal()` from `routes/builds.rs`

```rust
pub async fn insert_build_internal(
    state: &AppState,
    descriptor: BuildDescriptor,
    user_email: &str,
    priority: Priority,
    periodic_task_id: Option<&str>,
) -> Result<i64, BuildInsertError>
```

Shared by REST `submit_build` handler (wraps with permission checks)
and scheduler trigger (calls directly, bypasses permissions).

---

### REST API `routes/periodic.rs`

7 endpoints under `/api/periodic`:

| Method | Path | Cap required | Notes |
|---|---|---|---|
| POST | `/api/periodic` | `periodic:create` + `builds:create` (scoped) | Full validation |
| GET | `/api/periodic` | `periodic:view` | List all with computed `next_run` |
| GET | `/api/periodic/{id}` | `periodic:view` | 404 if not found |
| PUT | `/api/periodic/{id}` | `periodic:manage` (+ `builds:create` scoped if descriptor changed) | At least one field required |
| DELETE | `/api/periodic/{id}` | `periodic:manage` | 404 if not found |
| PUT | `/api/periodic/{id}/enable` | `periodic:manage` | Resets retry state |
| PUT | `/api/periodic/{id}/disable` | `periodic:manage` | Clears retry_at |

All mutation endpoints call `scheduler_notify.notify_one()` after DB
update.

Response shape (used by all endpoints returning task data):

```rust
struct PeriodicTaskResponse {
    id: String,
    cron_expr: String,
    tag_format: String,
    descriptor: serde_json::Value,
    priority: String,
    summary: Option<String>,
    enabled: bool,
    created_by: String,
    created_at: i64,
    updated_at: i64,
    retry_count: i64,
    retry_at: Option<i64>,
    last_error: Option<String>,
    last_triggered_at: Option<i64>,
    last_build_id: Option<i64>,
    next_run: Option<i64>,  // computed at response time
}
```

---

### `KNOWN_CAPS` update

Add `"periodic:create"`, `"periodic:view"`, `"periodic:manage"` to
`KNOWN_CAPS` in `permissions.rs`.

---

### AppState changes

```rust
pub scheduler_notify: Arc<tokio::sync::Notify>,
pub scheduler_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
```

---

## Implementation notes

- `croner` API verified: `Cron::from_str("0 2 * * *")` works for
  5-field crontab. Shorthands (`@daily`) supported. `dow=7` supported.
- `descriptor_version`: always 1. Incremented on future schema changes.
- PUT + active retry: clears retry state, new cron takes over.
- Scheduler re-fetches from DB at trigger time (not cached).
- `periodic_task_id` visible to `builds:list:own` users — deliberate,
  UUIDs are not secrets.
- `{S}` is typically `00` for cron fires, may be non-zero under retry.
- `.sqlx/` cache regenerated and committed.
