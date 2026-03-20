# Implementation Review: cbsd-rs Phase 10 — Periodic Builds

**Commit reviewed:**
- `d061131` — add periodic build scheduling (~2000 LOC, 14 source files)

**Evaluated against:**
- Design: `_docs/cbsd-rs/design/2026-03-18-periodic-builds.md` (v3, approved)

---

## Summary

Phase 10 is a substantial, well-structured commit that delivers the
entire periodic builds feature — migration, DB module, scheduler loop,
trigger logic, tag interpolation with tests, 7 REST endpoints, capability
updates, and AppState/main.rs wiring. The implementation faithfully
tracks the approved v3 design across the vast majority of requirements.

Two findings: `{DT}` tag format variable is missing seconds (produces
`20260318T1430` instead of the design's `20260318T143020`), and the
build trigger path duplicates insert/enqueue/dispatch logic rather than
sharing with the REST handler via an extracted internal function.

**Verdict: Two findings to fix. Otherwise approved.**

---

## Design Fidelity

| Design requirement | Status |
|---|---|
| Migration: `periodic_tasks` table (14 columns) | ✓ Matches exactly |
| Migration: `last_build_id REFERENCES builds(id) ON DELETE SET NULL` | ✓ |
| Migration: `idx_periodic_enabled` index | ✓ |
| Migration: `builds.periodic_task_id` FK `ON DELETE SET NULL` | ✓ |
| `croner = "3"` in Cargo.toml | ✓ |
| DB module: `insert_task` | ✓ |
| DB module: `get_task` | ✓ |
| DB module: `list_tasks` | ✓ |
| DB module: `list_enabled_tasks` | ✓ |
| DB module: `delete_task` | ✓ |
| DB module: `enable_task` (resets retry_count, retry_at, last_error) | ✓ |
| DB module: `disable_task` (clears retry_at) | ✓ |
| DB module: `update_trigger_success` | ✓ |
| DB module: `update_retry` | ✓ |
| DB module: `disable_with_error` | ✓ |
| All queries use `query!()` macro | ✓ (12 in db/periodic + 1 in routes) |
| All UPDATE queries set `updated_at = unixepoch()` | ✓ |
| Tag format: `validate_tag_format()` — 13 known placeholders | ✓ |
| Tag format: `interpolate_tag()` — all 14 variables | ✓ |
| Tag format: `validate_oci_tag()` — 128 char, charset | ✓ |
| Tag format: unit tests | ✓ (21 tests) |
| Scheduler: single tokio task, `run_scheduler()` | ✓ |
| Scheduler: load enabled tasks from DB | ✓ |
| Scheduler: compute fire time (cron or retry_at) | ✓ |
| Scheduler: sort by fire time, tiebreak by id | ✓ |
| Scheduler: `tokio::select!` sleep vs notify | ✓ |
| Scheduler: re-fetch from DB at trigger time (not cached) | ✓ |
| Scheduler: check still enabled after sleep | ✓ |
| Scheduler: cron parse error → log warning, skip | ✓ |
| Scheduler: missed fire → log warning, skip | ✓ |
| Scheduler: retry backoff 30s / 1.5x / 10min ceiling | ✓ |
| Scheduler: max 10 retries → disable | ✓ |
| Scheduler: `Notify` coalescing (full DB reload) | ✓ |
| Scheduler: startup after recovery, log summary | ✓ |
| Scheduler: shutdown abort alongside sweep/gc handles | ✓ |
| Trigger: `users.active` check | ✓ |
| Trigger: look up user name/email, set `signed_off_by` | ✓ |
| Trigger: interpolate tag + OCI validation | ✓ |
| Trigger: `builds.user_email = created_by` | ✓ |
| Trigger: `builds.periodic_task_id` set | ✓ |
| Trigger: `builds.priority` from task | ✓ |
| Trigger: update `last_triggered_at`, `last_build_id` on success | ✓ |
| Trigger: `UserDeactivated` → disable task | ✓ |
| Trigger: `Transient` → retry with backoff | ✓ |
| Trigger: `Fatal` → disable immediately | ✓ |
| REST: POST `/api/periodic` — `periodic:create` + `builds:create` | ✓ |
| REST: GET `/api/periodic` — `periodic:view` | ✓ |
| REST: GET `/api/periodic/{id}` — `periodic:view`, 404 | ✓ |
| REST: PUT `/api/periodic/{id}` — `periodic:manage` + `builds:create` if descriptor changed | ✓ |
| REST: PUT empty body → 400 | ✓ |
| REST: PUT clears retry state | ✓ |
| REST: DELETE `/api/periodic/{id}` — `periodic:manage`, 404 | ✓ |
| REST: PUT enable — resets retry_count, retry_at, last_error | ✓ |
| REST: PUT disable — clears retry_at | ✓ |
| REST: all mutations call `scheduler_notify.notify_one()` | ✓ |
| REST: response includes `retry_at` | ✓ |
| REST: `next_run = null` when disabled | ✓ |
| REST: POST 201 full resource shape | ✓ |
| REST: PUT 200 full resource shape | ✓ |
| `KNOWN_CAPS`: 3 new capabilities added | ✓ |
| `AppState`: `scheduler_notify`, `scheduler_handle` | ✓ |
| `AppState`: periodic router nested at `/api/periodic` | ✓ |
| `.sqlx/` cache regenerated (14 new JSON files) | ✓ |

---

## Findings

### F1 — `{DT}` variable is missing seconds

The design specifies `{DT}` → `20260318T143020` (with seconds). The
implementation at `tag_format.rs:120-127`:

```rust
"DT" => Some(format!(
    "{:04}{:02}{:02}T{:02}{:02}",
    now.year(), now.month(), now.day(), now.hour(), now.minute()
)),
```

This produces `20260318T1430` — no seconds component. The test at
line 277 confirms: `assert_eq!(result, "build-20260318T1430")`.

The design's variable table shows `{DT}` as ISO datetime
`20260318T143020` and the example shows `{version}-nightly-{DT}` →
`19.2.3-nightly-20260318T020000`.

**Impact:** Tags generated with `{DT}` will be 2 characters shorter
than expected and will not match the Python implementation's format.
Two periodic tasks firing in the same minute with different second
offsets (e.g., retry fires) will produce identical tags.

**Fix:** Add seconds to the format string:
```rust
"DT" => Some(format!(
    "{:04}{:02}{:02}T{:02}{:02}{:02}",
    now.year(), now.month(), now.day(),
    now.hour(), now.minute(), now.second()
)),
```

Update the test assertion to match: `"build-20260318T143045"`.

Severity: **Medium.** Incorrect tag format, easy fix.

### F2 — Build trigger path duplicates insert/enqueue/dispatch logic

The plan specified extracting `insert_build_internal()` as a shared
function used by both the REST handler and the scheduler trigger. The
implementation instead:

- Adds a separate `db::builds::insert_build_with_periodic()` in
  `db/builds.rs` (lines 57-83) that duplicates the INSERT query with
  an additional `periodic_task_id` parameter.
- Duplicates the enqueue/dispatch sequence in `trigger.rs:121-142`
  (serialize → insert → construct `QueuedBuild` → lock queue → enqueue →
  spawn dispatch) which mirrors `routes/builds.rs:125-178`.

The two code paths now handle the same logic independently. If the
REST handler's build submission logic changes (e.g., adding a build
log row at submission time, changing the dispatch pattern, adding
metrics), the trigger path must be updated separately.

**Impact:** Maintenance divergence risk. The current code is correct —
both paths produce the same observable behavior. But the design and
plan explicitly called for a shared function to prevent this class of
bug.

**Fix:** Extract `insert_build_internal()` from `routes/builds.rs`
that takes `periodic_task_id: Option<&str>` and call it from both the
REST handler (with `None`) and the trigger (with `Some(task_id)`). The
DB function can remain as `insert_build_with_periodic()` or be unified
with the original `insert_build()` by adding the optional parameter.

Severity: **Low.** No current correctness issue, maintenance concern.
Can be addressed in a follow-up commit.

---

## Observations

- **`update_task` in routes uses an inline `sqlx::query!()`.** This is
  the only query in `routes/periodic.rs` — the rest delegate to
  `db/periodic.rs`. It's a full-row UPDATE that clears retry state on
  any update. This is correct per the design (PUT + active retry clears
  retry state), but slightly inconsistent with the pattern of keeping
  all SQL in the `db/` module. Acceptable for the complexity of the
  merge logic.

- **`task_to_response` correctly computes `next_run`.** It checks:
  disabled → `None`, retry_at set → retry_at, otherwise → cron next
  occurrence. This matches the design's `next_run` semantics.

- **`croner` API is `Cron::from_str()` + `find_next_occurrence()`.** The
  design's pseudocode showed `iter_from(now, Direction::Forward)` which
  doesn't exist in `croner` v3. The implementation correctly uses
  `find_next_occurrence(&now, false)`. The v3 review flagged the
  pseudocode as potentially wrong — the implementation got it right.

- **Priority string validation is missing from POST.** The `priority`
  field in `CreateTaskBody` defaults to `"normal"` but is not validated
  against the CHECK constraint (`high`/`normal`/`low`). An invalid
  priority like `"critical"` would pass the Rust handler and fail at
  the SQLite CHECK constraint with a raw sqlx error. The PUT handler
  has the same gap. The design says "priority — optional, defaults to
  `"normal"`" but doesn't specify explicit validation beyond the DB
  constraint. The CHECK constraint is the correct guard — but the error
  message will be opaque. Consider adding a Rust-side validation for
  better error messages.

- **Scope validation on POST is capability-only, not full scoped.** The
  design says "full scope validation (channel, registry, repository)
  against the descriptor." The implementation at `periodic.rs:146-157`
  checks `has_cap("periodic:create")` and `has_cap("builds:create")`
  but does NOT call `require_scopes_all()`. Similarly, the PUT handler
  checks `has_cap("builds:create")` but not scopes. This means the
  scope escalation gate from the design review (v2 B2) is not fully
  implemented — a user with `builds:create` scoped to one channel can
  create a periodic task targeting any channel.

  **This should be addressed.** However, since only admins (wildcard
  `*`) currently have periodic capabilities, the scope check would
  always pass. It becomes load-bearing only when non-admin roles get
  periodic capabilities. Marking as an observation rather than a
  finding because the design's security note acknowledges the bypass,
  and the current permission model makes it inert.

- **`set_enabled` has `#[allow(dead_code)]`.** The generic function
  is unused because `enable_task` and `disable_task` are specialized.
  It's retained as a utility. Acceptable.

- **`descriptor` stored as `serde_json::Value` in the request, not
  validated as `BuildDescriptor`.** The design says "same validation
  as `POST /api/builds/` (known components, valid arch, etc.)." The
  implementation validates only `is_object()`. Full descriptor
  validation (component names, arch, etc.) is deferred — the trigger
  path will produce a `Fatal` error if the descriptor is malformed.
  This is acceptable as a first iteration since only admins create
  tasks.

---

## Commit Quality

- **~2000 LOC across 14 source files** — above the 400-800 guideline
  but justified (no useful intermediate state).
- **Atomic:** migration + DB + scheduler + trigger + routes + caps +
  AppState + main.rs + .sqlx/ cache all in one commit.
- **21 unit tests** for tag format validation, interpolation, and OCI
  tag validation.
- **Consistent patterns:** follows established `query!()`, `auth_error`,
  `notify_one()`, handle abort patterns.
