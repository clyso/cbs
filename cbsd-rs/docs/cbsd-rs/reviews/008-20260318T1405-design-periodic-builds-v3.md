# Design Review: Periodic Builds v3

**Document reviewed:**


- `cbsd-rs/docs/cbsd-rs/design/008-20260318T1412-periodic-builds.md` (v3, third revision)


**Cross-referenced against:**

- `cbsd-rs/cbsd-server/src/routes/builds.rs`
- `cbsd-rs/cbsd-server/src/routes/permissions.rs`
- `cbsd-rs/cbsd-server/src/app.rs`
- `cbsd-rs/cbsd-server/src/main.rs`
- `cbsd-rs/cbsd-server/Cargo.toml`
- `cbsd-rs/migrations/`

---

## Summary

The design has absorbed three rounds of review constructively. All 7
blockers and 7 major concerns from v1 and v2 are fully resolved. The
scheduler model, retry persistence, permission bypass semantics, scope
gating on PUT, and `signed_off_by` lifecycle are all correctly specified.

No blockers remain. No major concerns remain. The design is approved
for implementation. A handful of minor issues should be addressed in the
implementation — none require another review pass.

**Verdict: Approved for implementation.**

---

## Prior Findings Disposition

| Finding | Status |
|---|---|
| v1 B1 — Permission bypass as privilege escalation | Resolved |
| v1 B2 — cron crate absent + 7-field mismatch | Resolved |
| v1 B3 — Retry state not persisted | Resolved |
| v1 B4 — signed_off_by contradictory semantics | Resolved |
| v1 M1 — Scope validation missing at creation | Resolved |
| v1 M2 — created_by FK no ON DELETE | Resolved |
| v1 M3 — updated_at never updates | Resolved |
| v1 M4 — Missed-fire behavior unspecified | Resolved |
| v2 B1 — croner absent from Cargo.toml | Resolved |
| v2 B2 — PUT scope escalation path | Resolved |
| v2 B3 — retry_at missing from schema | Resolved |
| v2 M1 — last_build_id FK missing | Resolved |
| v2 M2 — builds.user_email attribution | Resolved |
| v2 M3 — Notify coalescing undocumented | Resolved |

---

## Blockers

None.

---

## Major Concerns

None.

---

## Minor Issues

- **Enable endpoint spec doesn't mention clearing `retry_at`.** The
  "Retry behavior" section (line ~260) says "Re-enabling resets
  `retry_count` to 0 and clears `retry_at`." The enable endpoint spec
  (line ~388) says "resets `retry_count` to 0, clears `last_error`" —
  `retry_at` is absent. An implementer reading only the endpoint spec
  will leave a stale `retry_at`, causing the re-enabled task to fire
  immediately instead of waiting for the next cron slot. **Fix:** Add
  "clears `retry_at`" to the enable endpoint spec. Same for disable.

- **`croner` v3 pseudo-code may not match actual API.** The design
  shows `use croner::{Cron, Direction}` and
  `cron.iter_from(chrono::Utc::now(), Direction::Forward).next()`. The
  `Direction` enum may have been removed in `croner` v3 — the actual
  API may be `Cron::new("...").parse()?.find_next_occurrence(...)` or
  similar. This is caught immediately by the compiler, not a runtime
  risk. **Fix:** Verify against published docs before implementation
  and update the code sample.

- **`retry_at` not in GET response shape.** The GET endpoints show
  `retry_count` and `last_error` but not `retry_at`. When a task is
  actively retrying, operators need to know when the next attempt fires.
  **Fix:** Add `retry_at` (nullable integer) to the response shape.

- **`descriptor_version` has no defined semantics.** The column exists
  with `DEFAULT 1` but the design doesn't state what triggers an
  increment or how the server uses it. Add a one-line note: "incremented
  when the `BuildDescriptor` schema changes; currently always 1."

- **`periodic_task_id` visible to `builds:list:own` users.** A user
  whose builds are triggered by the scheduler will see the periodic task
  UUID in their build records without `periodic:view`. UUIDs convey no
  actionable information — this is harmless. Add a sentence making it a
  deliberate decision.

- **`KNOWN_CAPS` update is mentioned but not called out as an explicit
  implementation step.** The design says "All three are added to
  `KNOWN_CAPS` in `permissions.rs`" — this should be in an
  implementation checklist or migration notes section to avoid being
  missed.

- **Build trigger path should share logic with `submit_build()`.** The
  design says "call the same build insertion + queue-enqueue logic."
  The implementation should extract a shared internal function rather
  than duplicating the logic, to prevent two-path divergence as
  `submit_build()` evolves.

- **Scheduler must read the DB row at trigger time, not a cached copy.**
  Between the scheduler waking and actually triggering, a REST mutation
  may have deleted or updated the task. The scheduler should re-fetch
  the task from DB at trigger time rather than using the priority queue's
  cached descriptor. This is an implementation concern the design could
  note.

- **`{S}` variable is not always `00` under retry.** Retries fire at
  `retry_at` timestamps that may have non-zero seconds. The note that
  `{S}` is always `00` is only true for cron-scheduled fires, not
  retry-scheduled fires. Amend the note or drop `{S}` from the variable
  table.

---

## Suggestions

- **`PUT /api/periodic/{id}/trigger`** for manual out-of-schedule
  firing. Common operational need — verify a task works without waiting
  for the next cron slot.

- **Clarify PUT + active retry interaction.** If `cron_expr` is updated
  while a task is retrying, state whether the retry is cancelled and the
  new cron schedule takes over, or the retry continues.

- **Add `.sqlx/` cache regeneration to implementation plan.** New tables
  and queries require `cargo sqlx prepare --workspace` and the updated
  cache committed atomically.

---

## Strengths

- **All 14 prior findings resolved** across 3 review passes with no
  regressions. The design has been iteratively refined to a high
  standard.

- **Single-scheduler task with `Notify` wakeup** — correct Tokio idiom,
  superior to Python's one-task-per-periodic-task model.

- **Retry state fully persisted** (`retry_count`, `retry_at`,
  `last_error`) with unambiguous startup semantics.

- **`users.active` check at trigger time** — correctly handles user
  deactivation months after task creation.

- **`signed_off_by` refreshed from users table** at trigger time —
  `{user}` resolves to current display name.

- **Full scope validation at creation AND on PUT descriptor updates** —
  prevents scope escalation through either path.

- **Permission model honestly documented** — the security note plainly
  states the bypass semantics and limitations.

- **`croner` crate specified with version, parsing API, and edge case
  handling** (leap years, shorthands, startup parse errors).

- **`periodic_task_id` on builds with `ON DELETE SET NULL`** and
  symmetric `last_build_id` FK — correct bidirectional traceability.

- **`Notify` coalescing documented as safe** — the full-scan reload
  invariant is explicitly stated.

- **Missed-fire skip with warning log** — correct default, avoids
  thundering-herd on restart.

- **Operational visibility** via `last_triggered_at`, `last_build_id`,
  `retry_count`, `last_error` columns.
