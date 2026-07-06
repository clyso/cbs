# Manual Trigger for Periodic Builds

## Problem

Periodic build tasks (design 008) fire only when their cron schedule elapses.
Operators need to fire an existing task **on demand**:

- to test a task's descriptor and tag format before enabling it (or after
  editing it), without waiting for the next cron fire;
- to re-run a task immediately after an external failure (registry outage, bad
  component) instead of waiting up to a day;
- to produce a nightly-style build early, occasionally at a different priority
  than the task's stored one.

There is no way to do this today short of editing the cron expression to "one
minute from now" and reverting it — error-prone and noisy.

## Design

### Overview

A new endpoint, `POST /api/periodic/{id}/trigger`, submits one build from an
existing periodic task immediately, with an optional one-shot priority override.
The build is attributed to the **requesting user**, not the task owner, and the
requester's own build permissions are validated — the manual trigger is "submit
a build using this task's template", not "impersonate the scheduler".

### Relationship to the scheduled trigger path (008)

The scheduler's `trigger_periodic_build()` (`scheduler/trigger.rs`) implements a
_delegation_ model: the build runs as the task owner (`created_by`), whose caps
and scopes are re-validated at fire time, and failures feed a retry/auto-disable
state machine.

The manual trigger deliberately does **not** reuse that function. It follows the
_request_ model of `POST /api/builds` (`routes/builds.rs::submit_build`):

| Aspect                | Scheduled fire (008)                       | Manual trigger (this doc)                |
| --------------------- | ------------------------------------------ | ---------------------------------------- |
| Actor                 | task owner (`created_by`)                  | requesting user                          |
| Caps checked          | owner: `periodic:create` + `builds:create` | requester: manage gate + `builds:create` |
| Scopes checked        | owner's channel/type scope only (see note) | requester's channel/type + repo scopes   |
| `signed_off_by`       | owner                                      | requester                                |
| `builds.user_email`   | owner                                      | requester                                |
| Owner `users.active`  | checked, disables task                     | not consulted                            |
| Failure handling      | retry/backoff/auto-disable                 | synchronous HTTP error, task untouched   |
| `enabled` requirement | enabled tasks only                         | enabled **or** disabled                  |

Note on the "Scopes checked" row: at fire time the scheduled path re-validates
only the channel/type scope (via `resolve_and_rewrite`); it does NOT re-check
repository scopes from `components[].repo` overrides — those are validated at
task creation/update time only. This falls short of audit-rem D3's "full stored
descriptor" re-validation language and is a pre-existing gap in
`scheduler/trigger.rs`, out of scope here and left un-fixed by this design. The
manual path has no such gap: it runs the full `submit_build`-equivalent scope
stack against the requester on every call.

Both paths converge on the same shared submission primitive,
`routes::builds::insert_build_internal()` (DB insert + queue enqueue + dispatch
attempt), and both stamp `builds.periodic_task_id` for traceability.

Rationale for requester attribution: the manual trigger is an interactive,
authenticated action. Attributing the build to the person who clicked keeps
audit attribution truthful and avoids widening 008's permission-bypass surface
(008's "security note on permission bypass" applies only to scheduler fires). It
also means an admin can safely fire a task whose owner has since been
deactivated — the build runs under the admin's identity and permissions.

### Authorization

The requester must satisfy **all** of:

1. **Manage gate** — `can_manage_task()` (`routes/periodic.rs`):
   `periodic:manage:any`, or `periodic:manage:own` with
   `task.created_by == user.email`. Denial returns 403 with the shared
   `PERIODIC_MANAGE_DENIED` message (no cap-miss vs ownership-miss leak, per
   audit-rem D3).
2. **`builds:create`** — the trigger submits a build, so the requester needs the
   same cap as `POST /api/builds`. 403 otherwise.
3. **Repository scopes** — `require_scopes_all` over
   `descriptor.components[].repo` overrides, exactly as `submit_build`.
4. **Channel/type scope** — enforced downstream by
   `channels::resolve_and_rewrite()` with the **requester's** user record.

`periodic:create` is NOT required: that cap gates creating new tasks; triggering
an existing task is a management action on that task. No new capability strings
are introduced; `KNOWN_CAPS` is unchanged.

There is no analogue of the scheduled path's "owner still active" check (its
step 1): the requester's account state is already enforced by the `AuthUser`
extractor, which rejects deactivated users before any handler runs.

Robot accounts may hold the manage caps and `builds:create`, so the robot guard
from `submit_build` applies unchanged: if the requester is a robot and the
resolved channel type's prefix template contains `${username}`, reject with 400.

### Endpoint

#### `POST /api/periodic/{id}/trigger` — Trigger a periodic task now

Registered in `routes/periodic.rs::router()` alongside `enable`/`disable`. The
sibling action endpoints (`/enable`, `/disable`) use PUT, but those are
idempotent state transitions; triggering is not idempotent (every call submits a
new build), so POST is the correct verb. The asymmetry is principled, not
accidental.

**Request body** (optional):

```json
{
  "priority": "high"
}
```

The handler parameter is typed `Option<Json<TriggerTaskBody>>` — the codebase's
first use of axum's `OptionalFromRequest` path for `Json`. This is load-bearing,
not stylistic: plain `Json<T>` (the pattern used by every existing handler in
`routes/periodic.rs`) rejects a request that carries no
`Content-Type: application/json` header with 415, which would break the bare
`curl -X POST .../trigger` case this endpoint exists to serve. `None` is treated
identically to `Some(Json(TriggerTaskBody { priority: None }))`.

Precisely: a request with **no** `Content-Type` header, or an `application/json`
body of `{}` or `{"priority": null}`, all mean "no override". One deliberate
edge case: a zero-length body sent WITH `Content-Type: application/json` is a
JSON syntax error (EOF) inside the extractor and yields axum's 400 — it is not
folded into the "no override" path. Documented, accepted behavior.

- `priority` — one-shot override for this submission only. One of `"high"`,
  `"normal"`, `"low"`. Any other value is rejected with 400 (strict, unlike the
  scheduler's lenient default-to-normal, because this is direct user input).
  When omitted, the task's stored `priority` is used. The stored task row is
  never modified by this field.

  The DTO field is `priority: Option<String>`, matched manually against the
  three literals — matching `CreateTaskBody.priority: String` /
  `UpdateTaskBody.priority: Option<String>` in the same file. Deliberately NOT
  `Option<cbsd_proto::Priority>`: a serde enum-variant mismatch inside `Json` is
  classified by axum as `JsonDataError` and rejected with **422**, which would
  contradict the 400 contract above; the manual match also keeps the
  strict-override vs lenient-stored-column split in one visible place.

**Flow:**

1. Load the task (404 if not found).
2. Authorization checks (above). The manage gate is checked first, against the
   loaded row, mirroring `update_task`/`delete_task`.
3. Parse the stored descriptor JSON into `BuildDescriptor`; re-validate via
   `components::validator::validate_descriptor` (WCP D5 — a component may have
   been removed since task creation). Failure → 400 with detail. The task is NOT
   disabled and its retry state is NOT touched (contrast with the scheduler's
   Fatal path) — the operator is present and sees the error synchronously;
   auto-disable is a protection for unattended fires only.
4. Overwrite `descriptor.signed_off_by` with the requester's name/email.
5. Interpolate `tag_format` with the current UTC time
   (`scheduler::tag_format::interpolate_tag`), validate the result with
   `validate_oci_tag` (400 on failure), and set `descriptor.dst_image.tag`.
   `{S}` will typically be non-zero here (manual fires are not minute-aligned) —
   this matches 008's documented retry behavior.

   Known inherited limitation: interpolation runs before channel resolution
   (step 6), in the same order as `scheduler/trigger.rs`. For a task whose
   stored descriptor omits `channel` (relying on the requester's default
   channel), the `{channel}` placeholder therefore interpolates to an empty
   string — on both the scheduled and manual paths. Kept as-is here so both
   paths produce identical tags for the same task; a reordering fix belongs to
   008/`tag_format.rs` scope, not this design. The response's `tag` field
   (below) makes the effect immediately visible to the operator.

6. `channels::resolve_and_rewrite(pool, &mut descriptor, requester_record)` —
   channel/type resolution, prefix rewrite, and channel scope check against the
   requester. Failure → 400.
7. Robot `${username}` prefix-template guard (as in `submit_build`) → 400.
8. Effective priority: request override if present, else task's stored
   `priority` parsed leniently (`"high"`/`"low"`/else `Normal`, matching the
   scheduler's mapping of the CHECK-constrained column).
9. `insert_build_internal(state, descriptor, requester_email, priority, Some(task_id), Some(channel_id), Some(channel_type_id))`.
10. Record bookkeeping: set `last_triggered_at = unixepoch()` and
    `last_build_id = <new id>` on the task (new DB helper, see below).
11. `tracing::info!` with task id, build id, requester `display_identity()`, and
    effective priority.

**Not done, deliberately:**

- **No scheduler notify.** The task's cron schedule and retry timers are
  unaffected; the scheduler's next-fire computation does not depend on
  `last_triggered_at`/`last_build_id`.
- **No retry-state mutation.** `retry_count`, `retry_at`, and `last_error` are
  scheduler bookkeeping. A successful manual fire does not prove the _scheduled_
  path is healthy (the failure may be owner-cap-related, which the manual path
  does not exercise), so it must not clear backoff state. An operator who wants
  to reset retry state re-enables the task (`PUT /{id}/enable` already does
  exactly that).
- **No `enabled` check or mutation.** Disabled tasks are triggerable — this is
  the primary "test before enable" use case, and it permits one-shot re-runs of
  tasks auto-disabled by retry exhaustion without re-arming the schedule.
- **No dedup.** Consistent with 008: if a scheduled fire is queued or running, a
  manual trigger submits a second build. A manual trigger can also race a
  concurrent cron fire; both builds are submitted. Documented known behavior.

**Response (202 Accepted)** — modeled on `POST /api/builds` (202 = build queued,
not yet built), but not field-identical to `SubmitBuildResponse`
(`id`/`state`/`is_robot`/`warning`): the id is renamed and two trigger-specific
fields are added.

```json
{
  "build_id": 42,
  "state": "queued",
  "tag": "19.2.3-nightly-20260706T181500",
  "priority": "high",
  "is_robot": false,
  "warning": "3 build(s) in queue"
}
```

- `build_id` — the new build (named `build_id`, not `id`, to avoid confusion
  with the task id in the request path).
- `state` — always `"queued"` at submission.
- `tag` — the interpolated destination tag, so the operator immediately knows
  what the run will produce.
- `priority` — the effective priority used.
- `is_robot` — whether the requester is a robot account; carried over from
  `SubmitBuildResponse` for the same client display-identity rendering reason.
- `warning` — optional, same semantics as `submit_build` (queue depth > 1).

**Errors:**

| Status | Condition                                                        |
| ------ | ---------------------------------------------------------------- |
| 400    | unknown `priority` string in body (handler's manual match)       |
| 400    | zero-length or malformed JSON body sent with a JSON content type |
| 400    | stored descriptor fails parse/validation                         |
| 400    | interpolated tag fails OCI validation                            |
| 400    | channel/type resolution or channel scope failure                 |
| 400    | robot requester with `${username}` prefix template               |
| 403    | manage gate denied (`PERIODIC_MANAGE_DENIED`)                    |
| 403    | requester lacks `builds:create`                                  |
| 403    | requester lacks a required repository scope                      |
| 404    | task id not found                                                |
| 415    | body sent with a non-JSON `Content-Type` (axum extractor)        |
| 422    | body fails deserialization, e.g. non-string `priority` (axum)    |
| 500    | database or queue insertion failure                              |

Handler-produced error bodies are the standard `{"detail": "..."}`
(`ErrorDetail`); the 400-EOF/415/422 rows are axum `Json`-extractor rejections
with axum's own plain-text bodies, consistent with every other JSON endpoint in
the server.

### Database

No schema migration. One new helper in `db/periodic.rs`:

```rust
/// Record a manual trigger: bookkeeping only. Unlike
/// `update_trigger_success`, retry state is left untouched.
pub async fn record_manual_trigger(
    pool: &SqlitePool,
    id: &str,
    build_id: i64,
) -> Result<(), sqlx::Error>
```

```sql
UPDATE periodic_tasks
SET last_triggered_at = unixepoch(), last_build_id = ?
WHERE id = ?
```

`last_triggered_at`/`last_build_id` thereby mean "last time this task produced a
build, by any path", which is the operationally useful reading (the fields are
pure visibility — nothing computes from them). A failure of this bookkeeping
UPDATE after a successful submission is logged at `error` level but does not
fail the request — the build exists and is queued; the response must report it.

`updated_at` is deliberately NOT touched. The scheduler's
`update_trigger_success` currently bumps `updated_at` on every scheduled fire,
conflating "definition last changed" with "last fired"; this design does not
replicate that conflation (`last_triggered_at` already carries the fire time),
and does not change the scheduler's existing behavior either — a cleanup there
is 008-scope follow-up material.

The new `sqlx::query!` requires regenerating the `.sqlx/` offline cache
(`cargo sqlx prepare --workspace -- --all-targets`).

### Concurrency

The handler runs concurrently with the scheduler task. Interactions:

- **Task row**: the handler reads the row once, then (after submission) performs
  a single-statement bookkeeping UPDATE. The scheduler's writes
  (`update_trigger_success`, `update_retry`, `disable_with_error`) touch retry
  columns the manual path never writes; `last_triggered_at` / `last_build_id`
  may be written by both, and last-writer-wins is acceptable for visibility-only
  fields.
- **Task deleted/updated mid-flight**: the handler operates on its loaded
  snapshot. A concurrent delete leaves `builds.periodic_task_id` dangling → the
  FK is `ON DELETE SET NULL`, and the bookkeeping UPDATE matches zero rows. Both
  are benign.
- **Queue**: `insert_build_internal` already serializes queue access via the
  queue mutex; nothing new.

### OpenAPI

Standard utoipa integration: `#[utoipa::path]` on the handler (tag `periodic`,
`security(("bearer" = []), ("cookie" = []))`), new `TriggerTaskBody` and
`TriggerTaskResponse` DTOs with `ToSchema`, handler registered via
`routes!(trigger_task)`. Spec collection is automatic.

Because the body is optional, the shorthand `request_body = TriggerTaskBody`
must not be used: utoipa computes `requestBody.required` solely from whether the
**content type** is `Option<...>` (verified against utoipa-gen 5.4.0 — the
shorthand vs parenthesized form makes no difference), so the content type itself
must be wrapped:

```rust
request_body(content = Option<TriggerTaskBody>, description = "...")
```

Verify the emitted `requestBody.required` is `false` at implementation time.

### cbc CLI

New subcommand in `cbc/src/periodic.rs`:

```
cbc periodic trigger <ID> [--priority high|normal|low]
```

- `<ID>` accepts a UUID prefix, resolved via the existing `resolve_periodic_id`
  helper (same as `get`/`update`/`delete`).
- `--priority` parsed with the existing `builds::parse_priority`; omitted → no
  `priority` field in the request body (server uses the task's stored priority).
- Sends `POST periodic/{id}/trigger` via `CbcClient::post`; prints the resulting
  build id, tag, effective priority, and the warning if present.
- Non-2xx surfaces the server's `detail` via the existing `Error::Api` decoding.

### Capabilities summary (delta to 008's table)

No new capabilities. The trigger endpoint requires:

| Endpoint                          | Caps                                                                                             |
| --------------------------------- | ------------------------------------------------------------------------------------------------ |
| `POST /api/periodic/{id}/trigger` | (`periodic:manage:own` with ownership, or `periodic:manage:any`) + `builds:create` (with scopes) |

### Testing

Unit tests in `routes/periodic.rs` and `db/periodic.rs`, mirroring existing
patterns (`test_support::{test_pool, test_app_state, auth_user}`; seeded users
via `db::users::create_or_update_user` where the handler loads the user record):

- authorization matrix: manage-any / manage-own(owner) / manage-own(other) /
  no-manage-cap / manage-but-no-builds:create → 403 paths.
- disabled task triggers successfully; `enabled` remains 0.
- priority: override respected; omitted body → stored priority; invalid value →
  400; stored row's `priority` column unchanged after override.
- retry state untouched: seed a task with `retry_count`/`retry_at`/ `last_error`
  set, trigger, assert all three unchanged while
  `last_triggered_at`/`last_build_id` are updated.
- 404 unknown id; 400 invalid stored descriptor.
- `record_manual_trigger` DB test: updates the two bookkeeping columns and
  nothing else (including `updated_at`).
- build row assertions: `periodic_task_id = task id`, `user_email = requester`,
  priority string as expected.

**HTTP-layer tests (mandatory, not optional).** The codebase's handler-test
convention calls handler functions directly, bypassing axum's `FromRequest`
layer entirely — a direct-call test of the omitted-body case would pass
regardless of whether the extractor is `Json<T>` or `Option<Json<T>>`, proving
nothing. The optional-body contract MUST therefore be exercised through the real
router, using the `build_router(state, session_layer).oneshot(request)` pattern
already used by `app.rs` tests. Note these requests go through the full auth
stack, so the tests must set up a real credential (seeded user + role and a
valid bearer token or API key) — the implementation plan must budget for that
setup:

- `POST /api/periodic/{id}/trigger` with **no body and no `Content-Type`
  header** → 202, task's stored priority used.
- same with `Content-Type: application/json` and body `{}` → 202.
- same with body `{"priority": "urgent"}` → 400.
- same with `Content-Type: text/plain` and a body → 415.
