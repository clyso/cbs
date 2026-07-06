# 024 — Manual Trigger for Periodic Builds: Implementation Plan

**Design:** `docs/cbsd-rs/design/024-20260706T1815-periodic-manual-trigger.md`

**Reviews:**

- `docs/cbsd-rs/reviews/024-20260706T1828-design-periodic-manual-trigger-v1.md`
- `docs/cbsd-rs/reviews/024-20260707T0909-design-periodic-manual-trigger-v2.md`
  (verdict: go, 95/100)

## Scope

Add `POST /api/periodic/{id}/trigger`: fire an existing periodic task on demand,
with an optional one-shot priority override, plus the matching
`cbc periodic trigger` subcommand. The build is attributed to the **requesting
user** (manage gate + `builds:create` with full scope validation), converging on
the existing `insert_build_internal()` path. The task's schedule, `enabled`
flag, and retry state are never touched; only
`last_triggered_at`/`last_build_id` are updated. No schema migration, no new
capability strings.

## Commit Breakdown

3 commits. Auto-generated `.sqlx/` and `Cargo.lock` do not count toward the
authored-LOC estimates.

| #   | Commit                                                            | ~LOC | Status |
| --- | ----------------------------------------------------------------- | ---- | ------ |
| 1   | `cbsd-rs/docs: add manual periodic-trigger design, plan, reviews` | docs | Done   |
| 2   | `cbsd-rs/server: manual trigger endpoint for periodic tasks`      | ~650 | Done   |
| 3   | `cbsd-rs/cbc: add 'periodic trigger' subcommand`                  | ~180 | Done   |

> The `plans/README.md` phase table lapsed after Phase 13 / design 017;
> consistent with plan 020's decision, no partial row is added here.

---

### Commit 1: `cbsd-rs/docs: add manual periodic-trigger design, plan, reviews`

**Documentation only.** Already landed as the design commit; this plan and its
reviews are folded into the same commit (message adjusted when they land).

| File                                                              | Change     |
| ----------------------------------------------------------------- | ---------- |
| `docs/cbsd-rs/design/024-…-periodic-manual-trigger.md`            | New        |
| `docs/cbsd-rs/reviews/024-…-design-periodic-manual-trigger-v1.md` | New        |
| `docs/cbsd-rs/reviews/024-…-design-periodic-manual-trigger-v2.md` | New        |
| `docs/cbsd-rs/plans/024-20260709T0049-periodic-manual-trigger.md` | New (this) |
| `docs/cbsd-rs/reviews/024-…-plan-periodic-manual-trigger-v…md`    | New        |

---

### Commit 2: `cbsd-rs/server: manual trigger endpoint for periodic tasks` (~650)

The endpoint itself, end-to-end: an operator can fire a periodic task via the
REST API after this commit. All behavior per the design's "Endpoint" section;
the notes below are implementation mechanics only.

| File                                     | Change                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cbsd-server/src/db/periodic.rs`         | New `record_manual_trigger(pool, id, build_id)`: single `sqlx::query!` UPDATE of `last_triggered_at`/`last_build_id` only (no retry columns, no `updated_at`). Unit tests in the same module.                                                                                                                                                                                                                                                  |
| `cbsd-server/src/routes/periodic.rs`     | New `TriggerTaskBody { priority: Option<String> }` and `TriggerTaskResponse { build_id, state, tag, priority, is_robot, warning }` DTOs (`ToSchema`); new `trigger_task` handler with `Option<Json<TriggerTaskBody>>` extractor implementing the design's 11-step flow; `routes!(trigger_task)` registration; `#[utoipa::path(post, path = "/{id}/trigger", request_body(content = Option<TriggerTaskBody>, …), …)]`. Unit + HTTP-layer tests. |
| `cbsd-rs/docs/cbsd-rs/design/README.md`  | Add the trigger endpoint row to the "Periodic Builds" REST API surface table (current-state index, kept accurate).                                                                                                                                                                                                                                                                                                                             |
| `cbsd-server/src/routes/test_support.rs` | New shared helper `seed_authed_bearer(pool, email, caps) -> String`: seed the user, a role with the given caps and a wildcard channel scope, mint a PASETO bearer, insert its hash; returns the raw token. Used by all four HTTP-layer tests.                                                                                                                                                                                                  |

Implementation mechanics:

- The handler follows `submit_build`'s sequence for the requester-side work
  (caps → validator → repo scopes → `signed_off_by` overwrite → user record →
  `resolve_and_rewrite` → robot `${username}` guard → `insert_build_internal`),
  and `update_task`'s fetch-first pattern for the manage gate. Reuse
  `scheduler::tag_format::{interpolate_tag, validate_oci_tag}` (already `pub`).
- Priority: manual match on the override (`"high" | "normal" | "low"` → 400
  otherwise); fall back to the stored column via the scheduler's lenient
  mapping. The literal is validated **fail-fast**, immediately after the cap
  checks and before descriptor parsing or channel resolution — this refines
  (does not contradict) the design's step 8, which only computes the effective
  priority, and it keeps the `"urgent" → 400` HTTP-layer test independent of the
  component/channel fixture below.
- `record_manual_trigger` failure after successful submission: log at `error`,
  still return 202 (per design).
- New sqlx query ⇒ regenerate `.sqlx/`
  (`cargo sqlx prepare --workspace -- --all-targets`), commit the cache.

**Tests** (from the design's Testing section, all in this commit):

- `db/periodic.rs`: `record_manual_trigger` updates exactly the two bookkeeping
  columns (assert `retry_count`/`retry_at`/`last_error`/ `updated_at`/`enabled`
  unchanged).
- `routes/periodic.rs` direct-call handler tests: authorization matrix
  (manage-any / manage-own-owner / manage-own-other / no-manage-cap /
  manage-without-builds:create), 404 unknown id, 400 invalid stored descriptor,
  400 unknown priority string, disabled task fires with `enabled` still 0, retry
  state untouched, build row assertions (`periodic_task_id`, `user_email` =
  requester, priority string), stored `priority` column unchanged after an
  override.
- HTTP-layer tests through `build_router(state, session_layer)` +
  `tower::ServiceExt::oneshot` (mandatory per design — the direct-call
  convention bypasses `FromRequest` and cannot verify the optional-body
  contract). Cases: no body + no `Content-Type` → 202; `{}` with JSON content
  type → 202; `{"priority": "urgent"}` → 400; `text/plain` body → 415. These are
  the codebase's first authenticated `oneshot` tests; the two pieces of setup
  they need:
  - **Auth** — via the new `test_support::seed_authed_bearer` helper (one place,
    not inlined 4×): `db::users::create_or_update_user`, a role via
    `db::roles::create_role` + `set_role_caps_and_scopes` (caps + wildcard
    channel scope) + `add_user_role`, then
    `auth::paseto::token_create(email, ttl, &"0".repeat(128))` (matching
    `test_support`'s `token_secret_key`) and
    `db::tokens::insert_token(pool, hash, email, expires_at)`; requests send
    `Authorization: Bearer <raw>`.
  - **Resolvable descriptor fixture** (202 cases only — the 400/415 cases fail
    before it is consulted, given the fail-fast priority check above): build the
    state with `test_app_state_with_components_dir` + `temp_component_dir` so
    `validate_descriptor` passes, and seed a channel + type via
    `db::channels::create_channel` / `create_type` / `set_default_type`, with
    the task's stored descriptor naming that channel. No such fixture exists in
    the codebase yet; its cost is inside this commit's LOC estimate.

Pre-existing symbols touched: none modified — `insert_build_internal`,
`can_manage_task`, `tag_format::*` are called, not changed. (GitNexus impact
check still run before edit per repo policy.)

---

### Commit 3: `cbsd-rs/cbc: add 'periodic trigger' subcommand` (~180)

CLI parity: the same capability from `cbc`.

| File                  | Change                                                                                                                                                                                                                                                                                                                                                                                                      |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cbc/src/periodic.rs` | New `PeriodicCommands::Trigger { id, priority: Option<String> }` variant + `cmd_trigger`: resolve the id prefix via `resolve_periodic_id`, validate `--priority` client-side with `builds::parse_priority`, `POST periodic/{id}/trigger` (body `{}` or `{"priority": …}`), print `build_id`, `tag`, effective `priority`, and `warning` if present. Unit tests for arg parsing if the module has precedent. |
| `cbc/src/builds.rs`   | Only if needed: make `parse_priority` reachable (it is already `pub fn`).                                                                                                                                                                                                                                                                                                                                   |

Note (from the design): `CbcClient::post` always sends a JSON body, so `cbc`
never exercises the bodyless path — omitted `--priority` sends `{}`.

---

## Verification

- Per commit: `cargo fmt --all`, `cargo clippy --workspace`,
  `cargo check --workspace`, `cargo test --workspace` — all clean, zero
  warnings.
- Commit 2:
  `DATABASE_URL=sqlite:///tmp/cbsd-dev.db cargo sqlx prepare --workspace -- --all-targets`,
  then `SQLX_OFFLINE=true cargo build --workspace`; `.sqlx/` changes included.
  Manually inspect `/api/docs/openapi.json` (or the spec test output) to confirm
  the trigger path's `requestBody.required` is `false`.
- Manual end-to-end (dev mode,
  `podman-compose -f podman-compose.cbsd-rs.yaml up`): create a task via
  `cbc periodic new`, leave it disabled;
  `cbc periodic trigger <id> --priority high` → build queued at high priority,
  `cbc periodic get <id>` shows `last_build_id`/`last_triggered_at` set,
  `enabled` still false, `retry_count` 0; `curl -X POST` with no body against
  the same endpoint → 202.

## Notes

- Update the Status column in the Commit Breakdown table after each commit
  lands.
- Impl-stage adversarial review may add fixups against commits 2/3; the review
  docs are folded into the commit they review.
