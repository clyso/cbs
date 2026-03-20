# Design Review: Periodic Builds v2

**Document reviewed:**
- `_docs/cbsd-rs/design/2026-03-18-periodic-builds.md` (revised)

**Cross-referenced against:**
- `cbsd-rs/cbsd-server/src/routes/builds.rs`
- `cbsd-rs/cbsd-server/src/routes/permissions.rs`
- `cbsd-rs/cbsd-server/src/app.rs`
- `cbsd-rs/cbsd-server/src/main.rs`
- `cbsd-rs/cbsd-server/Cargo.toml`
- `cbsd-rs/migrations/`
- `cbsd/cbslib/core/periodic.py`

---

## Summary

The revised design resolves all 4 blockers and all 4 major concerns from
the v1 review. The permission model is now honestly documented, retry
state is persisted, `signed_off_by` semantics are unambiguous, cron
timezone is stated, and `updated_at` management is specified. The design
is substantially improved.

Three new issues require resolution: the `croner` crate is still absent
from `Cargo.toml` and its field-count behavior needs verification; PUT
on a periodic task's descriptor creates a privilege escalation path via
the scope bypass; and `retry_at` is referenced in the scheduler loop
but not stored in the schema.

**Verdict: Approve with conditions.** Fix the 3 items below, then
proceed to implementation without another full review pass.

---

## Prior Findings Disposition

| v1 Finding | Status |
|---|---|
| B1 — Permission bypass as privilege escalation | Resolved — security note documents the bypass as intentional; `users.active` check added; full scope validation at creation time |
| B2 — cron crate absent + 7-field mismatch | Partially resolved — switched to `croner`; dependency still not in Cargo.toml |
| B3 — Retry state not persisted | Resolved — `retry_count` and `last_error` columns added |
| B4 — `signed_off_by` contradictory semantics | Resolved — trigger step 4 overwrites from users table lookup |
| M1 — Scope validation missing at creation | Resolved — full scope validation at creation explicitly stated |
| M2 — `created_by` FK no ON DELETE | Resolved — documented as "users deactivated, never deleted" invariant |
| M3 — `updated_at` never updates | Resolved — application-layer update in PUT handler specified |
| M4 — Missed-fire behavior unspecified | Resolved — skipped with warning log at startup |

---

## Blockers

### B1 — `croner` crate absent from `Cargo.toml` and field-count behavior unverified

The design names `croner` as the cron-parsing crate. It is not in
`cbsd-rs/cbsd-server/Cargo.toml`. Additionally, `croner 0.7.x` may use
a **6-field format by default** (seconds-level precision) rather than
the standard 5-field crontab the design specifies. If the implementer
passes `"0 2 * * *"` and the crate interprets `0` as seconds and `2`
as minutes, every nightly build fires at 00:02 instead of 02:00.

**Fix:** Add `croner` to `Cargo.toml` with a pinned version. Verify
that the specific version supports standard 5-field crontab out of the
box. Include the exact parsing API call in the design (e.g.,
`Cron::new("0 2 * * *").parse()`) and confirm it returns
`chrono::DateTime<Utc>` compatible with the scheduler's sleep
computation. If `croner` requires a 6-field format, either prepend a
wildcard seconds field internally before parsing, or switch to a crate
that natively supports 5-field.

### B2 — `PUT /api/periodic/{id}` descriptor update creates a scope escalation path

The design requires only `periodic:manage` for PUT. If a
`periodic:manage` user (who may lack `builds:create` scope for the
target channel) updates the descriptor to target a different channel,
the scheduler fires it unchecked — the only gate is `users.active` on
the original creator.

Concrete attack path:
1. Alice has `builds:create` scoped to `channel=ces-devel`, creates a
   periodic task.
2. Bob has `periodic:manage` but no `builds:create`.
3. Bob PUTs a new descriptor targeting `channel=prod`.
4. Scheduler fires — bypasses permission checks — build runs against
   `channel=prod`.

**Fix:** `PUT /api/periodic/{id}` must require `builds:create` (with
full scope validation against the updated descriptor) in addition to
`periodic:manage`. This ensures descriptor mutations are gated by the
updater's build scopes. Document this explicitly.

### B3 — `retry_at` referenced in scheduler loop but not in schema

The scheduler loop says: "compute `retry_at` with backoff" and "A task
in retry has `retry_at` as its next fire time." But the schema has only
`retry_count` and `last_error` — no `retry_at` column. On restart with
`retry_count > 0`, the scheduler cannot determine the remaining backoff
time. It will either fire immediately (losing the backoff) or
recompute from scratch (adding unexpected delay).

**Fix:** Add `retry_at INTEGER` to the schema (nullable, set when
`retry_count > 0`, cleared on success or disable). On startup, if
`retry_at > unixepoch()`, use it as the next fire time. If
`retry_at <= unixepoch()`, fire immediately (backoff elapsed during
downtime). Two-line schema addition that resolves an entire ambiguity
class.

---

## Major Concerns

### M1 — `last_build_id` has no FK to `builds(id)`

The schema declares `last_build_id INTEGER` with no FK constraint. The
same migration adds `builds.periodic_task_id TEXT REFERENCES
periodic_tasks(id) ON DELETE SET NULL` — the reverse direction has the
FK but the forward direction does not. This is an inconsistency.

**Fix:** Add `REFERENCES builds(id) ON DELETE SET NULL` to
`last_build_id`. Consistent with the existing pattern.

### M2 — `builds.user_email` attribution for periodic builds unspecified

When the scheduler submits a build internally, `builds.user_email`
drives `builds:list:own` filtering. The design does not specify what
value is stored. If it's the `created_by` email (correct), state it.
If it's a system account or null, the `builds:list:own` query will
produce incorrect results.

**Fix:** Add one sentence: "`builds.user_email` for periodic builds is
set to `periodic_tasks.created_by`."

### M3 — `Notify` coalescing should be documented as safe

Multiple REST mutations while the scheduler processes a trigger may
coalesce into a single wakeup. This is safe because the reload is a
full DB scan — but the design should state this invariant explicitly.

**Fix:** Add: "Because the reload at step 2 is a full scan of
`periodic_tasks`, a single wakeup after a burst of mutations converges
to correct state. Multiple concurrent `notify_one()` calls may coalesce
into a single reload, which is acceptable."

---

## Minor Issues

- **`croner` day-of-week behavior.** The design says `0` and `7` are
  Sunday. Verify this matches `croner`'s actual behavior — some crates
  reject `7`.

- **POST 201 response missing operational fields.** The creation
  response omits `retry_count`, `last_error`, `last_triggered_at`,
  `last_build_id`, `created_at`, `updated_at`. Returning the full
  resource shape (as the list response does) avoids an immediate
  follow-up GET.

- **PUT success status code not specified.** The pattern in the existing
  codebase uses 200. Document it explicitly.

- **`descriptor_version` semantics undefined.** What triggers an
  increment? Who sets it on PUT? If it matches the `builds` table
  semantics, say so.

- **`{S}` (seconds) variable is always `00`.** Cron is minute-granular.
  Including `{S}` in a tag format is misleading — add a note.

- **Python `@daily`/`@weekly` shorthands.** If operators are migrating
  from the Python cbsd which uses `croniter` (supports shorthands),
  will those expressions need reformatting? State whether shorthands
  are in or out of scope.

- **`summary` null semantics on PUT.** Is `{"summary": null}` a clear
  operation or a no-op? Specify patch semantics for nullable fields.

- **`periodic:manage` vs `periodic:create` orthogonality.** State
  explicitly whether these are independent or `periodic:manage` is a
  superset.

- **Missing index on `enabled`.** Add
  `CREATE INDEX idx_periodic_enabled ON periodic_tasks(enabled)` in
  the migration.

- **`periodic_task_id` visible to `builds:list:own` users.** A user
  can enumerate periodic task UUIDs by inspecting their build history
  without `periodic:view`. Likely harmless (UUIDs reveal nothing) but
  should be a deliberate decision.

- **`croner` parse error for existing DB tasks at startup.** If the
  crate is updated and a previously valid expression becomes invalid,
  should the scheduler log-and-skip or abort? Specify the error
  handling.

---

## Suggestions

- **`PUT /api/periodic/{id}/trigger`** endpoint for manual trigger
  outside of schedule. Common operational need.

- **Deduplication mitigation advice.** The design documents "no
  deduplication" as known behavior. Add a note suggesting operators
  check `GET /api/builds?state=queued&periodic_task_id=<uuid>` to
  detect stacking builds.

- **Scheduler priority queue data structure.** The design says "priority
  queue (sorted by next-fire-time)" — calling out `BinaryHeap` with a
  wrapper struct (for `Ord` on the tiebreaker) saves the implementer a
  minor surprise.

- **`last_retry_at` column.** Storing when the last retry *attempt*
  occurred (vs. when the next retry is scheduled) is more useful for
  operators reading the DB directly.

---

## Strengths

- **All 8 prior findings resolved.** The design has improved
  substantially through two review passes with no regressions.

- **Single-scheduler task with `Notify` wakeup** is the correct Tokio
  idiom — cleaner than Python's one-task-per-periodic-task model.

- **Retry state persisted to DB.** Survives restarts. Python kept all
  state in memory.

- **`users.active` check at trigger time.** Correctly handles user
  deactivation months after task creation. Python had no equivalent.

- **`signed_off_by` refreshed from users table at trigger time.**
  Ensures `{user}` resolves to current display name, not stale snapshot.

- **Missed-fire skip with warning log** — correct default, avoids
  thundering-herd on restart.

- **Full scope validation at creation time** — channel, registry,
  repository scopes checked against the requesting user.

- **Permission model honestly documented.** The security note plainly
  states the bypass semantics.

- **`periodic_task_id` on builds table with `ON DELETE SET NULL`** —
  correct traceability without coupling build history to task lifecycle.

- **`priority` field on periodic tasks** — triggered builds use the
  configured priority, not hardcoded Normal.

- **OCI tag validation after interpolation** — catches overly long or
  malformed tags before submission.

---

## Open Questions

1. **`croner` version and API:** Which exact version and parsing call?
   Does it support 5-field natively?

2. **`retry_at` storage:** Will it be added to the schema per B3?

3. **PUT scope requirement:** Will `builds:create` with full scope
   validation be required on PUT per B2?

4. **`builds.user_email` attribution:** Confirmed as
   `periodic_tasks.created_by`?

5. **Startup parse error handling:** Log-and-skip or abort for invalid
   cron expressions in existing DB tasks?
