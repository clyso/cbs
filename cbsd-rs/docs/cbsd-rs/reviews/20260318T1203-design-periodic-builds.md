# Design Review: Periodic Builds

**Document reviewed:**
- `_docs/cbsd-rs/design/2026-03-18-periodic-builds.md`

**Cross-referenced against:**
- `cbsd-rs/cbsd-server/src/routes/builds.rs` (build submission, scope checks)
- `cbsd-rs/cbsd-server/src/routes/permissions.rs` (KNOWN_CAPS, scope logic)
- `cbsd-rs/cbsd-server/src/app.rs` (AppState)
- `cbsd-rs/cbsd-server/src/main.rs` (startup sequence, task handles)
- `cbsd-rs/cbsd-server/Cargo.toml` (dependencies)
- `cbsd-rs/migrations/` (existing migrations)
- `cbsd/cbslib/core/periodic.py` (Python implementation)
- `cbsd-rs/cbsd-server/src/queue/mod.rs` (build queue)
- `cbsd-rs/cbsd-server/src/ws/dispatch.rs` (dispatch logic)

---

## Summary

The design is structurally sound — the single-scheduler-task model with
a priority queue and `Notify`-based wakeup is the correct Tokio idiom
and a genuine improvement over the Python one-task-per-periodic-task
approach. The retry behavior, permission modeling, and shutdown handling
are all well-specified. However, the design has **4 blockers**: the
permission bypass at trigger time is a permanent privilege escalation
that survives role revocation and user deactivation; the `cron` crate
is absent from `Cargo.toml` and its 7-field default parsing will
interpret all 5-field expressions incorrectly; retry state is not
persisted, so a permanently failing task never auto-disables across
server restarts; and `signed_off_by` has contradictory semantics as
both a template variable source and a write field on the build
submission path.

**Verdict: Revise and re-review.** Fix the 4 blockers, then the design
is approvable.

---

## Blockers

### B1 — Permission bypass is a permanent privilege escalation

The design says "bypass permission checks" at trigger time because "the
admin already had permission at creation time." This is a TOCTOU
vulnerability. After task creation, the user's `builds:create` role can
be revoked, their scopes narrowed, or their account deactivated — the
task continues firing indefinitely under the now-revoked identity,
bypassing all current RBAC state.

The scope dimension makes this concrete: if a builder's `builds:create`
is scoped to `channel=ces-public` and an admin creates a periodic task
on their behalf targeting `channel=ces-internal`, the task fires outside
the builder's scope. Even if the scope gap is fixed later, the task is
unaffected.

**Fix:** Choose one explicitly:
- **Option A (stronger):** At trigger time, re-validate `created_by`
  user: check `users.active`, check `builds:create` capability, and run
  `require_scopes_all()` against the descriptor. Disable the task if
  validation fails.
- **Option B (accepted trade-off):** Keep the bypass, but at minimum
  check `users.active` at trigger time. Add an explicit security note:
  "periodic tasks represent a permanent delegation of the creator's
  permissions at creation time." Document that revoking a user's roles
  does NOT stop their periodic tasks — an admin must manually disable
  them.

### B2 — `cron` crate absent from `Cargo.toml` and field-count mismatch

The `cron` crate is not in either `Cargo.toml`. Additionally,
`cron 0.12`'s `Schedule::from_str` expects a **7-field** Quartz-style
expression (`seconds minutes hours day-of-month month day-of-week year`),
not the 5-field standard crontab the design specifies.
`Schedule::from_str("0 2 * * *")` will either reject the expression or
interpret `0` as seconds and `2` as minutes — making what the user
intended as "02:00 daily" fire at "00:02:00 daily."

**Fix:** Add the dependency to `Cargo.toml`. Specify in the design the
exact parsing approach: either prepend a wildcard seconds field
internally (`"0 0 2 * * *"` with trailing `*` for year) before parsing,
or evaluate an alternative crate (`croner`, `saffron`) that natively
supports 5-field syntax. Document this decision — it affects what values
are valid at the API level and what gets stored in the DB.

### B3 — Retry state is not persisted across server restarts

The retry loop runs in memory. On restart, the scheduler reloads from
DB and computes next fire times from `cron_expr`, resetting all retry
state. A permanently failing task (bad registry URL, unknown component)
will: fire → fail → disable → operator re-enables → server restarts →
retry count resets → repeat forever.

**Fix:** Add two columns to `periodic_tasks`:
- `retry_count INTEGER NOT NULL DEFAULT 0`
- `retry_until INTEGER` (nullable — NULL when not in retry)

At startup, if `retry_until > unixepoch()`, the scheduler sleeps until
`retry_until` rather than the next cron time. Auto-disable when
`retry_count` reaches a configured max. Reset to 0 on success.

### B4 — `signed_off_by` has contradictory semantics

The tag format table lists `{user}` → `descriptor.signed_off_by.user`
as a template variable. But in `builds.rs:122-123`, `submit_build`
unconditionally overwrites `signed_off_by` with the authenticated
user's identity. The design says the periodic path calls "the same
build insertion + queue-enqueue logic" but doesn't state whether
`signed_off_by` is overwritten at trigger time.

If NOT overwritten: `{user}` resolves to whatever arbitrary string
the creator put in the original descriptor body.
If overwritten: requires a DB lookup for the `created_by` user's
current name.

**Fix:** Add one sentence: "Before submission,
`descriptor.signed_off_by.user` and `.email` are set to the
`created_by` user's current `name` and `email` from the `users` table."
This makes `{user}` deterministic and audit attribution consistent.

---

## Major Concerns

### M1 — Scope validation missing at creation time

Even if B1 is resolved with Option B (bypass at trigger time), creation
time MUST validate the requesting user's full scope set against the
descriptor — not just `has_cap("builds:create")`. The current design
validates only the capability name, not the associated scope constraints.
Without this, any user with `builds:create` scoped to one channel can
create a periodic task targeting any channel.

**Fix:** At `POST /api/periodic`, perform the full scope validation
from `submit_build`: validate channel, registry, and repository scopes
from the descriptor against the requesting user's assignments.

### M2 — `created_by REFERENCES users(email)` has no ON DELETE behavior

With `PRAGMA foreign_keys = ON`, deleting a user who owns periodic
tasks produces a FK constraint error. The design doesn't specify the
intended behavior.

**Fix:** Choose one: (a) no ON DELETE clause + document that user
deletion requires cleaning up their periodic tasks first, or (b)
`ON DELETE CASCADE` if tasks are user-owned assets. Apply consistently
with `workers.created_by`.

### M3 — `updated_at` column will never update

SQLite has no auto-update timestamps. The column defaults to
`unixepoch()` at creation but no trigger or application code is
specified to update it. Clients seeing `updated_at == created_at`
after a PUT update is misleading.

**Fix:** Either add a SQLite trigger or specify that application code
explicitly sets `updated_at = unixepoch()` in every UPDATE query
(matching the existing pattern for `users.updated_at`).

### M4 — Missed-fire behavior during downtime is unspecified

The `cron` crate's `upcoming()` skips past times. A nightly build
that should have fired while the server was down is silently skipped.
No log entry, no notification.

**Fix:** Add: "If a task's computed next fire is in the past, the
missed fire is skipped and the task is scheduled for its next future
occurrence. Log a warning at startup for each missed fire."

---

## Minor Issues

- **No `periodic_task_id` in `builds` table.** No way to query "all
  builds spawned by task X." Consider adding a nullable FK
  `periodic_task_id TEXT REFERENCES periodic_tasks(id) ON DELETE SET NULL`.

- **`periodic:create`, `periodic:view`, `periodic:manage` not in
  `KNOWN_CAPS`.** Non-admin roles will get "unknown capability" errors.
  Add to `KNOWN_CAPS` in `routes/permissions.rs`.

- **Priority of triggered builds is unspecified.** No `priority` field
  in `periodic_tasks`. All triggered builds implicitly use `Normal`.
  Consider adding `priority TEXT NOT NULL DEFAULT 'normal'`.

- **No deduplication for overlapping triggers.** If a build from the
  previous trigger is still queued/active when the next cron fires, a
  second build is submitted. State explicitly as known behavior.

- **PUT empty body should return 400.** "All fields optional" should
  still require at least one field.

- **`next_run` type is `integer | null`.** The Rust response struct
  should use `Option<i64>`.

- **Constant `tag_format` footgun.** `tag_format = "latest"` produces
  the same tag every trigger, causing tag collisions. Add a note
  recommending at least one time-varying variable.

- **DELETE response 200 vs 204.** Current API convention uses 200 with
  JSON body. Consistent but worth a note.

- **`descriptor_version` absent.** The builds table has this column;
  `periodic_tasks` does not. Future `BuildDescriptor` schema changes
  may break stored descriptors silently.

- **OCI tag length/charset validation.** Tags are limited to 128 chars
  and `[a-zA-Z0-9_][a-zA-Z0-9_.-]*`. Tag format interpolation can
  exceed this. Add a validation check.

- **Scheduler startup position.** Should start after
  `run_startup_recovery`, matching `sweep_handle` and `gc_handle`.
  State position in startup sequence explicitly.

- **Scheduler handle storage.** Confirm it follows the
  `Arc<Mutex<Option<JoinHandle<()>>>>` pattern from `sweep_handle`
  and `gc_handle`, and that the abort path in `main.rs` is extended.

---

## Suggestions

- **Store `last_triggered_at` and `last_build_id`** in `periodic_tasks`
  for operational visibility.

- **State cron timezone explicitly:** "All cron expressions are evaluated
  in UTC. Local-time scheduling is not supported."

- **Document `Notify` semantics:** Specify whether the implementation
  uses `notify_waiters()` or `notify_one()`, and whether the scheduler
  re-checks for pending notifications before sleeping.

- **Log a startup summary:** At scheduler start, log the number of
  enabled tasks and their next fire times.

- **`PUT /api/periodic/{id}` and re-enable:** State explicitly that PUT
  does not change `enabled` state. Operators must call `PUT .../enable`
  separately after fixing a disabled task.

- **Full `version_create_helper` parameter listing.** The tag format
  interpolation section documents descriptor-based variables. Consider
  also documenting the complete set of descriptor fields that are
  preserved verbatim vs. overwritten at trigger time.

---

## Strengths

- **Single-scheduler task with `Notify` wakeup** is the correct Tokio
  idiom — cleaner, cheaper, and more deterministic than
  one-task-per-periodic-task.

- **Retry constants match Python implementation faithfully** (30s / 1.5x
  / 10-min ceiling). Correct and complete port.

- **Validation at creation time** for both `cron_expr` and `tag_format`
  prevents delayed failures.

- **Permission modeling is explicitly conservative** — `periodic:*`
  restricted to admin, `builder` excluded.

- **Shutdown behavior is correctly scoped** — scheduler is stateless
  beyond the DB, no special cleanup needed.

- **`descriptor` stored as JSON** is consistent with the existing
  `builds` table pattern.

- **`AppState.scheduler_notify: Arc<tokio::sync::Notify>`** is the
  correct mechanism — cheaper than a channel, idiomatic Tokio.

---

## Open Questions

1. **Two tasks with same next-fire-time** — what is the trigger
   ordering? Deterministic (e.g., by ID) or arbitrary?

2. **Should `PUT /api/periodic/{id}` accept `enabled` as a field?**
   Currently requires separate enable/disable endpoints. Intentional
   asymmetry?

3. **Trigger fires but all workers disconnected** — build queues
   normally and dispatches when a worker reconnects? State explicitly.

4. **How does the scheduler distinguish retry sleep from normal cron
   sleep** in the single-loop model? If retry state is added (B3),
   trace the interaction.

5. **Leap-year cron expressions** (`0 0 29 2 *`) — what does the
   chosen crate do? Warn or reject?

6. **`periodic_task_id` visibility** — if added to `builds` table,
   does a user with `builds:list:own` see it? Does this leak info
   about periodic tasks they lack `periodic:view` for?
