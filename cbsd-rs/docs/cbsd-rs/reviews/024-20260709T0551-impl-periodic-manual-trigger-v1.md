# Review: Manual Trigger for Periodic Builds — Implementation (024) — v1

Commits reviewed:

- `1e754171` `cbsd-rs/server: manual trigger endpoint for periodic tasks`
- `62487e96` `cbsd-rs/cbc: add 'periodic trigger' subcommand`
- (`1c8e31c3` is a docs-only fixup to the plan's progress table; not in scope
  for this review.)

Design implemented:
`cbsd-rs/docs/cbsd-rs/design/024-20260706T1815-periodic-manual-trigger.md` (v2
verdict: go, 95/100).

Plan implemented:
`cbsd-rs/docs/cbsd-rs/plans/024-20260709T0049-periodic-manual-trigger.md` (v1
verdict: go-with-conditions, 75/100 — conditions S1/S2/S3).

Source verified against: `cbsd-server/src/routes/periodic.rs`,
`cbsd-server/src/db/periodic.rs`, `cbsd-server/src/routes/builds.rs`
(`submit_build`, `insert_build_internal`),
`cbsd-server/src/scheduler/trigger.rs`,
`cbsd-server/src/scheduler/tag_format.rs`, `cbsd-server/src/channels/mod.rs`,
`cbsd-server/src/auth/extractors.rs`, `cbsd-server/src/auth/paseto.rs`,
`cbsd-server/src/routes/auth.rs`, `cbsd-server/src/routes/test_support.rs`,
`cbsd-server/src/config.rs`, `cbsd-proto/src/build.rs`, `cbc/src/periodic.rs`,
`cbc/src/builds.rs`, and `git log`/`git show` for both commits in full.
Verification also included running the workspace's format, lint, test, and
sqlx-offline-cache checks directly (results below), not just reading the diffs.

## Executive Summary

This is an exceptionally faithful implementation. Every numbered step, error
code, DTO shape, and "not done, deliberately" item in the design is present in
the code exactly as specified, including the subtle ones (fail-fast strict
priority match before descriptor work, tag interpolation before channel
resolution, `record_manual_trigger` touching only two columns, non-fatal
bookkeeping failure, requester — not owner — attribution, disabled tasks
remaining triggerable, no scheduler notify). All three conditions from the
plan's v1 review (S1 fixture strategy, S2 priority-validation ordering, S3
shared auth helper) were resolved correctly and are visible in the diff exactly
as the plan's amendments promised. `cargo fmt --all --check`,
`SQLX_OFFLINE=true cargo clippy --workspace`,
`SQLX_OFFLINE=true cargo test --workspace` (304 tests across `cbsd-server` and
`cbsd-worker`), and `cargo sqlx prepare --workspace --check -- --all-targets`
all pass clean. The only findings are two minor test-hygiene items with no
correctness, security, or spec impact. **Go.**

## Verification Performed

```
cargo fmt --all --check --manifest-path cbsd-rs/Cargo.toml
  → clean, no output

SQLX_OFFLINE=true cargo clippy --workspace --manifest-path cbsd-rs/Cargo.toml
  → Finished, zero warnings

SQLX_OFFLINE=true cargo test --workspace --manifest-path cbsd-rs/Cargo.toml
  → cbsd-server: 238 passed; 0 failed
  → cbsd-worker: 66 passed; 0 failed

env -C cbsd-rs DATABASE_URL=sqlite:///tmp/cbsd-dev-review.db \
  cargo sqlx migrate run
  → 10/10 migrations applied cleanly

env -C cbsd-rs DATABASE_URL=sqlite:///tmp/cbsd-dev-review.db \
  cargo sqlx prepare --workspace --check -- --all-targets
  → exits clean; offline cache is up to date (3 new .sqlx/*.json files
    committed, consistent with the query text — see "sqlx cache" below)
```

## Design-Fidelity Checklist

Every item the task explicitly asked to verify, checked against the actual diff
(`cbsd-server/src/routes/periodic.rs:1240-1440` for the handler):

| Contract item                                                                               | Verified                                                                                                                                                                                               |
| ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Authorization order: 404 → manage-gate 403 → `builds:create` 403 → priority 400 (fail-fast) | Yes — exact order in `trigger_task`, matching the plan's S2 amendment                                                                                                                                  |
| `Option<Json<TriggerTaskBody>>` extractor (not plain `Json<T>`)                             | Yes, with the exact rationale comment from the design                                                                                                                                                  |
| Strict priority match (400) vs lenient stored-column fallback                               | Yes — manual `match` on `"high"/"normal"/"low"`, `_ => 400`; fallback uses the scheduler's `_ => Normal` lenient mapping                                                                               |
| `signed_off_by` overwritten with requester's name/email                                     | Yes                                                                                                                                                                                                    |
| `builds.user_email` = requester (not owner)                                                 | Yes, verified via `insert_build_internal(..., &user.email, ...)` and the `trigger_disabled_task_succeeds...` test's DB assertion                                                                       |
| Repo scope checks against requester (`require_scopes_all`)                                  | Yes, identical block to `submit_build`'s                                                                                                                                                               |
| Robot `${username}` guard                                                                   | Yes, identical block to `submit_build`'s, using `user_record.is_robot`                                                                                                                                 |
| Tag interpolation before channel resolution                                                 | Yes — `tag_format::interpolate_tag` + `validate_oci_tag` run before `channels::resolve_and_rewrite`                                                                                                    |
| `record_manual_trigger` touches only `last_triggered_at`/`last_build_id`                    | Yes — single-column `UPDATE`, verified by `record_manual_trigger_updates_only_bookkeeping_columns` asserting `retry_count`/`retry_at`/`last_error`/`updated_at`/`enabled`/`priority` are all unchanged |
| Bookkeeping failure is logged, non-fatal                                                    | Yes — `tracing::error!` with `task_id` field, request still returns 202                                                                                                                                |
| 202 response shape (`build_id`/`state`/`tag`/`priority`/`is_robot`/`warning`)               | Yes, field-for-field                                                                                                                                                                                   |
| `utoipa` `request_body(content = Option<TriggerTaskBody>, ...)`                             | Yes, plus a dedicated `trigger_openapi_body_is_not_required` test asserting `requestBody.required != true` in the generated spec                                                                       |
| No scheduler notify, no retry-state mutation, no `enabled` mutation                         | Yes — confirmed by reading the full diff of `db/periodic.rs` and `scheduler/trigger.rs` (latter untouched)                                                                                             |
| Disabled tasks are triggerable                                                              | Yes — `trigger_disabled_task_succeeds_and_preserves_task_state` seeds `enabled = 0` and asserts success + `enabled` still `false` afterward                                                            |

All four mandatory HTTP-layer (`oneshot`) tests from the design are present and
pass: no-body/no-`Content-Type` → 202, `{}` with JSON content type → 202,
`{"priority": "urgent"}` → 400, `text/plain` body → 415.

## Plan Conditions (S1/S2/S3) — Resolution Check

- **S1 (fixture strategy)** — Resolved. `seed_trigger_fixture` overrides
  `AppState.components` via struct-update (satisfying `validate_descriptor`) and
  seeds a real `channels`/`channel_types` row pair via
  `create_channel`/`create_type`/`set_default_type` (satisfying
  `resolve_and_rewrite`). This is exactly the fixture the plan review said was
  missing, now built and used by all four HTTP-layer tests and the two
  success-path handler tests.
- **S2 (priority-validation ordering)** — Resolved and pinned down in code, not
  just narrative: the strict priority match runs immediately after the
  `builds:create` check, before descriptor parsing. The
  `trigger_unknown_priority_is_400` test's doc comment states this explicitly
  ("validated strictly and fail-fast ... so no fixture beyond the task row") and
  the test itself uses a bare `test_app_state(pool)` with no component/channel
  fixture — proving the fail-fast path, not merely asserting it.
- **S3 (shared auth helper)** — Resolved.
  `test_support::seed_authed_bearer(pool, email, caps)` is exactly the helper
  the review recommended (seeds user, scope-less role, mints a token, inserts
  its hash) and is reused by all four `oneshot` tests instead of being inlined
  four times.

## Collateral Fix: PASETO Test Key Length

The commit changes `test_support::test_server_config()`'s `token_secret_key`
from `"0".repeat(128)` (128 hex chars → 64 bytes) to `"0".repeat(64)` (64 hex
chars → 32 bytes, matching `docs/cbsd-rs/plans/deployment.md`'s documented
`openssl rand -hex 32` format and `SymmetricKey::<V4>::from`'s 32-byte
requirement in `auth/paseto.rs`). This was a real, previously-latent bug: no
test before this commit ever exercised `token_create`/`token_decode` through
`test_server_config()` (the `paseto.rs` unit tests use their own correctly-sized
`TEST_KEY` constant; `routes/auth.rs`'s existing `handler_tests` never reach the
OAuth callback's token-issuance code path). The fix is correctly scoped, has no
effect on any other passing test (confirmed via grep across the crate for other
consumers and by the full `cargo test --workspace` run above showing 0
regressions), and is properly called out as a stated side effect in the commit
message.

## Commit Hygiene (`git-commits` skill)

Both commits pass the five-point smell test:

- **Commit 1e754171** (~828 insertions across 4 authored files, excluding the 3
  auto-generated `.sqlx/*.json` files): one-sentence purpose ("add the trigger
  endpoint"); previous commit (docs-only) compiles trivially; revertable without
  breaking `enable`/`disable`/etc; testable (curl-able endpoint after landing);
  no dead code — every new DTO, helper, and handler has a caller in the same
  commit. Slightly above the ~650 LOC estimate but within the 400-800 guideline
  band and not a candidate for splitting (DB helper + handler + tests are one
  inseparable capability).
- **Commit 62487e96** (~90 lines): one-sentence purpose ("CLI parity for the
  trigger endpoint"); compiles on top of commit 1; revertable independently;
  testable (two new serialization unit tests); no dead code. Comfortably within
  band.

Both messages are Ceph-style, DCO-signed, carry exactly one `Co-authored-by`
trailer, and are not GPG-signed, matching this repository's established
convention.

## Minor Findings

### M1 — Retry-state seed helper duplicated across two test modules

`db::periodic::tests::seed_task_in_retry` (`db/periodic.rs:389-410`) and
`routes::periodic::tests::seed_trigger_task` (`routes/periodic.rs:341-365`) both
perform the same sequence — seed a user, `insert_task`, then a raw
`UPDATE periodic_tasks SET enabled = 0, retry_count = 3, retry_at = ?, last_error = 'boom'`
with the identical literal `retry_at` value (`4_102_444_800`) and the identical
`last_error` string (`"boom"`). They live in different files/modules
(crate-privacy makes cross-module test-fn reuse mildly inconvenient), but this
is exactly the kind of repeated fixture logic the design's own commit just
extracted once already (`seed_authed_bearer` in `test_support.rs` for the
auth-setup recipe). A
`test_support::seed_task_in_retry_state(pool, task_id, ...)` helper would remove
the duplication the same way. Not a production-code risk — test-only — and not a
new pattern this repository was already avoiding elsewhere (no prior precedent
of this specific db/routes seed-helper split existed to violate), but worth
cleaning up opportunistically.

### M2 — "manage-own(owner)" is not exercised by a dedicated trigger-handler test

The plan's Commit 2 section explicitly names five combinations for the
"authorization matrix": manage-any / manage-own(owner) / manage-own(other) /
no-manage-cap / manage-without-builds:create. Three of the five denial cases are
directly tested (`trigger_denied_without_manage_cap`,
`trigger_denied_for_own_cap_on_other_task`,
`trigger_denied_without_builds_create`); the manage-any success case is
exercised (indirectly but concretely) by
`trigger_disabled_task_succeeds_and_preserves_task_state`, whose
`REQUESTER_CAPS` is `["periodic:manage:any", "builds:create"]`. The
manage-own(owner) **success** case has no dedicated `trigger_task`-level test —
it is covered only by the pre-existing, unrelated pure-function test
`own_cap_holder_can_manage_own_task` (`can_manage_task` in isolation, not the
full handler). This exactly matches this file's existing convention for
`update_task`/`delete_task`/`enable_task`/ `disable_task`, none of which have a
handler-level manage-own(owner) success test either — so this is not a new
regression relative to the codebase's own house style, but it is a small,
checkable gap relative to what the plan explicitly promised to test for this
specific feature.

## Suggestions (non-blocking)

- The cbc commit message describes the omitted `--priority` flag as serializing
  "to an empty body" — the actual behavior (confirmed by
  `trigger_body_without_priority_serializes_to_empty_object`) is an empty JSON
  _object_ (`{}`), sent with a JSON `Content-Type`, not a zero-length body. The
  design is explicit that these are different things server-side (a zero-length
  body with `Content-Type: application/json` is a 400 EOF error, not "no
  override"). The commit message's phrasing is a harmless imprecision, not a
  code issue — cbc's actual request always carries `{}`, matching the design's
  own note that "`cbc` never exercises the bodyless path."
- `cmd_trigger` sends the parsed `Priority` enum directly
  (`TriggerPeriodicBody { priority: Option<Priority> }`) rather than following
  `cmd_update`'s "validate then discard, send the raw string" idiom the plan
  cited. This is arguably cleaner (guarantees exactly one of the three canonical
  lowercase strings reaches the wire, with no case-sensitivity risk) and is
  verified by `trigger_body_priority_serializes_lowercase`. Noted as a
  deliberate, reasonable divergence from the plan's stated idiom, not a defect.

## Confidence Scoring

| Item                                                                                                                                              | Points | Description                                                                                                                                                         |
| ------------------------------------------------------------------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                                                                                    | 100    |                                                                                                                                                                     |
| M1 — duplicated retry-state seed helper across `db/periodic.rs` and `routes/periodic.rs` test modules                                             | −5     | Test-only duplication (D2, reduced severity: no production impact, no prior repo pattern being violated, ~20 lines each)                                            |
| M2 — "manage-own(owner)" success path has no dedicated `trigger_task`-level test, despite being named in the plan's explicit authorization matrix | −5     | Mild plan-coverage gap (D1, reduced severity: matches this file's existing convention for sibling handlers; the underlying gate logic is independently well-tested) |
| **Total**                                                                                                                                         | **90** |                                                                                                                                                                     |

Per the interpretation scale (90-100: "Ready to merge. Minor or no issues."),
this implementation is ready to merge as-is. Both deductions are test-hygiene
nits with no bearing on correctness, security, or design fidelity — every
authorization check, every error code, every "not done, deliberately" item, and
both HTTP-layer contract edge cases are implemented exactly as designed and
verified to pass.

## Findings Ordered by Severity

1. 🟢 **M1** (minor) — `seed_task_in_retry` (`db/periodic.rs`) and
   `seed_trigger_task` (`routes/periodic.rs`) duplicate the same retry-state
   seeding boilerplate across two test modules; could be extracted to
   `test_support.rs`.
2. 🟢 **M2** (minor) — the plan's explicit "manage-own(owner)"
   authorization-matrix entry has no dedicated handler-level success test in
   this commit (covered only by the pre-existing pure-function `can_manage_task`
   test), consistent with — not worse than — this file's existing convention for
   sibling handlers.
3. Suggestion (non-blocking) — commit message for 62487e96 says "empty body"
   where the actual, verified behavior is an empty JSON object (`{}`) sent with
   a JSON content type.
4. Suggestion (non-blocking) — `cmd_trigger` sends a typed `Priority` enum
   rather than following `cmd_update`'s "discard and resend raw string" idiom;
   this is a reasonable, verified improvement, not a defect.

## Verdict

**Go.** No blockers, no correctness or security defects, no spec deviations.
`cargo fmt`, `cargo clippy`, `cargo test --workspace` (304 tests), and
`cargo sqlx prepare --workspace --check -- --all-targets` all pass clean against
the actual worktree. The implementation matches the approved design and plan
with unusually high fidelity, including every subtle ordering and "not done,
deliberately" detail, and correctly resolves all three conditions (S1/S2/S3)
from the plan's v1 review. The two minor findings (M1, M2) are optional cleanup,
not merge conditions.
