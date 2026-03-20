# Plan Review: Phase 10 — Periodic Builds

**Plan reviewed:**
- `cbsd-rs/docs/cbsd-rs/plans/008-20260318T1713-periodic-builds.md`

**Evaluated against:**
- Design: `cbsd-rs/docs/cbsd-rs/design/008-20260318T1412-periodic-builds.md` (v3, approved)
- Current codebase: `routes/builds.rs`, `app.rs`, `main.rs`, `permissions.rs`

---

## Summary

The plan faithfully tracks the approved v3 design. Every design
requirement is accounted for. The single-commit rationale is sound —
the feature has no useful intermediate state. The file list, DB module
functions, scheduler loop, trigger sequence, tag format functions,
REST endpoints, and extracted `insert_build_internal()` all match the
design precisely.

**Verdict: Approved. Good to go.**

---

## Design Fidelity

| Design requirement | Plan coverage |
|---|---|
| Migration: `periodic_tasks` table (all 14 columns) | ✓ DDL matches exactly |
| Migration: `builds.periodic_task_id` FK | ✓ `ON DELETE SET NULL` |
| Migration: `idx_periodic_enabled` index | ✓ |
| `croner = "3"` in Cargo.toml | ✓ |
| DB module: 10 functions using `query!()` | ✓ |
| Tag format: validate, interpolate, OCI check | ✓ (3 functions + unit tests) |
| Scheduler: single tokio task, priority queue, Notify | ✓ |
| Scheduler: re-fetch from DB at trigger time (not cached) | ✓ (step 5) |
| Scheduler: startup after recovery, log summary | ✓ |
| Scheduler: shutdown alongside sweep/gc handles | ✓ |
| Scheduler: missed fires → log warning, skip | ✓ (step 2) |
| Scheduler: parse failure → log warning, skip | ✓ (step 2) |
| Trigger: `users.active` check | ✓ (step 1) |
| Trigger: look up user name/email, set `signed_off_by` | ✓ (steps 2-3) |
| Trigger: tag interpolation + OCI validation | ✓ (step 4) |
| Trigger: `insert_build_internal()` with `periodic_task_id` | ✓ (step 5) |
| Trigger: `builds.user_email = created_by` | ✓ (step 5) |
| Trigger: `builds.priority` from task | ✓ (step 5) |
| Trigger: update `last_triggered_at`, `last_build_id` | ✓ (step 6) |
| Retry: transient → increment, compute `retry_at`, persist | ✓ |
| Retry: max 10 → disable | ✓ |
| Retry: non-transient → disable immediately | ✓ |
| Retry: user deactivated → disable | ✓ |
| REST: 7 endpoints with correct capabilities | ✓ |
| REST: POST requires `periodic:create` + `builds:create` (scoped) | ✓ |
| REST: PUT requires `periodic:manage` + `builds:create` if descriptor changed | ✓ |
| REST: enable resets retry state (including `retry_at`) | ✓ |
| REST: disable clears `retry_at` | ✓ |
| REST: PUT empty body → 400 | ✓ ("at least one field required") |
| REST: nullable field semantics (`summary: null` clears) | Not explicit in plan |
| REST: full response shape with `retry_at` | ✓ (in struct) |
| REST: `next_run: Option<i64>` (null when disabled) | ✓ |
| `KNOWN_CAPS`: 3 new capabilities | ✓ |
| `periodic:create` and `periodic:manage` independence | ✓ (plan table matches) |
| `AppState`: `scheduler_notify`, `scheduler_handle` | ✓ |
| Extract `insert_build_internal()` from `routes/builds.rs` | ✓ |
| `.sqlx/` cache regenerated | ✓ |
| `croner` API verified for 5-field, shorthands, dow=7 | ✓ (implementation notes) |
| PUT + active retry: clears retry state | ✓ (implementation notes) |
| `{S}` typically 00, non-zero under retry | ✓ (implementation notes) |
| `periodic_task_id` visible to `builds:list:own` | ✓ (implementation notes) |
| `descriptor_version` always 1 | ✓ (implementation notes) |

---

## Observations

**The `insert_build_internal()` extraction is the right call.** The
current `submit_build` handler (lines 73-172 of `builds.rs`) interleaves
permission checks with build insertion, enqueuing, and dispatch. The
plan correctly identifies that the shared internal function takes
`descriptor`, `user_email`, `priority`, and `periodic_task_id`, covering
both the REST and scheduler paths. The DB `insert_build` function will
need a new `periodic_task_id` parameter (or the migration's
`ALTER TABLE builds ADD COLUMN` is handled separately). Either way, the
plan accounts for it.

**The `PeriodicTaskResponse` struct includes `retry_at`.** This
addresses the v3 review's minor issue about `retry_at` being missing
from the GET response.

**The scheduler loop step 5 ("re-fetch task from DB") addresses the
stale-cache concern** from the v3 review. Good.

**The implementation notes section addresses all remaining v3 review
minor items** — `croner` API verified, PUT + retry interaction, `{S}`
under retry, `periodic_task_id` visibility, `descriptor_version`.

**One minor gap in the plan:** The design specifies nullable field
semantics for PUT (`"summary": null` clears the field vs. omitting
leaves unchanged). The plan's PUT endpoint says "at least one field
required" but doesn't explicitly state the null-clears-field semantics.
This is a minor implementation detail that the design covers — the
plan doesn't contradict it, it just doesn't restate it.

**Single-commit justification is sound.** The feature truly has no
useful intermediate state. ~1100 LOC is above the guideline but the
changes follow established patterns (CRUD endpoints, `query!()` macros,
tokio task loop). The alternative — splitting into dead-code commits —
is worse.

---

## No Issues Found

The plan is a faithful, complete translation of the approved design into
an implementation specification. Every design requirement is covered.
The file list is complete. The function signatures are concrete. The
scheduler loop matches. The trigger sequence matches. The REST endpoints
match including capability requirements and scope validation.
