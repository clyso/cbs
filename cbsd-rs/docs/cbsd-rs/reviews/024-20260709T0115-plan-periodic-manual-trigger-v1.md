# Review: Manual Trigger for Periodic Builds — Implementation Plan (024) — v1

Plan reviewed:
`cbsd-rs/docs/cbsd-rs/plans/024-20260709T0049-periodic-manual-trigger.md` (part
of docs commit `83e6241`, HEAD of branch `wip/cbsd-rs-periodic-retry`).

Design implemented by this plan:
`cbsd-rs/docs/cbsd-rs/design/024-20260706T1815-periodic-manual-trigger.md` (v2
verdict: go, 95/100 —
`cbsd-rs/docs/cbsd-rs/reviews/024-20260707T0909-design-periodic-manual-trigger-v2.md`).
The design decisions themselves are settled and out of scope for this review;
this review judges only whether the plan faithfully and completely implements
them, with sound commit boundaries.

Source verified against: `cbsd-server/src/db/periodic.rs`,
`cbsd-server/src/routes/periodic.rs`, `cbsd-server/src/routes/builds.rs`,
`cbsd-server/src/scheduler/trigger.rs`,
`cbsd-server/src/scheduler/tag_format.rs`, `cbsd-server/src/channels/mod.rs`,
`cbsd-server/src/db/channels.rs`, `cbsd-server/src/auth/extractors.rs`,
`cbsd-server/src/auth/paseto.rs`, `cbsd-server/src/db/tokens.rs`,
`cbsd-server/src/db/roles.rs`, `cbsd-server/src/db/users.rs`,
`cbsd-server/src/routes/test_support.rs`, `cbsd-server/src/app.rs`,
`cbsd-server/src/components/mod.rs`, `cbsd-server/src/components/validator.rs`,
`cbsd-server/src/db/robots.rs`, `cbc/src/periodic.rs`, `cbc/src/builds.rs`,
`cbc/src/client.rs`, `migrations/005_channels.sql`, and `git log` for
commit-split precedent (design 008's implementation history).

## Executive Summary

The plan faithfully translates the approved design into a 3-commit sequence with
sound boundaries (docs / server+DB+tests / cbc), and every function, type,
constant, and signature it cites — `insert_build_internal`, `can_manage_task`,
`PERIODIC_MANAGE_DENIED`, `resolve_and_rewrite`, `interpolate_tag`,
`validate_oci_tag`, `prefix_template_contains_username`, `parse_priority`,
`resolve_periodic_id`, `token_create`, `insert_token`, `create_role`,
`set_role_caps_and_scopes`, `add_user_role`, `create_or_update_user`,
`test_session_layer` — was independently verified to exist with exactly the
claimed signature and visibility. Zero factual errors were found in the plan's
code citations, and the N1 OpenAPI fix from the design's v2 review is correctly
reflected in the current design text the plan implements. However, the plan's
HTTP-layer test section (mandatory per the design, not optional) omits a real
and concrete cost: `test_app_state()`'s hardcoded empty `components: Vec::new()`
field, combined with the total absence anywhere in this codebase of a test that
seeds a resolvable channel/channel-type pair, means the two mandatory 202 cases
(and possibly the 400-bad-priority case, depending on exactly where in the
11-step flow the priority literal is validated) cannot pass against a bare
periodic-task fixture. The implementer will have to invent new DB/state fixture
machinery mid-commit that the plan neither describes nor budgets for. This is
fixable without touching the design and does not block starting implementation,
but it should be spelled out in the plan (or accepted as an explicit known gap)
before the commit-2 test-writing phase begins, so it is not discovered the hard
way via mysteriously-failing "should be 202" tests.

## Critical Issues 🔴

None. Nothing found rises to a correctness defect, security gap, or spec
deviation that blocks implementation.

## Significant Concerns 🟡

### S1 — HTTP-layer test fixture strategy is unspecified and has no precedent anywhere in this codebase

**Problem:** The plan's mandatory HTTP-layer tests (`no body → 202`, `{} → 202`,
`{"priority": "urgent"} → 400`, `text/plain → 415`) run through
`build_router(state, session_layer)` + `tower::ServiceExt::oneshot`, using a
real `AppState`. Two of those four cases require the handler to actually reach
`insert_build_internal` successfully (202), and the third likely does too (see
S2). But:

- `test_app_state()` / `test_app_state_with_components_dir()`
  (`routes/test_support.rs`) both hardcode `components: Vec::new()`.
  `components::validator::validate_descriptor` (verified,
  `components/validator.rs:49-62`) rejects **any** descriptor with a non-empty
  `components` list unless every component name is present in `known` — with
  `known` empty, every non-trivial periodic-task descriptor fails at design step
  3 with `UnknownComponent`, and an empty-components descriptor fails with
  `EmptyComponents`. There is no way to reach a 202 through `test_app_state()`
  as written; the test must override `components` (workable via Rust
  struct-update syntax since `AppState`'s fields and `ComponentInfo`'s fields
  are all `pub` — verified — but this workaround is not mentioned anywhere in
  the plan).
- Reaching `insert_build_internal` also requires `channels::resolve_and_rewrite`
  (design step 6) to succeed, which requires a real, joinable `channels` +
  `channel_types` row pair in the DB (`migrations/005_channels.sql`:
  `channel_types.channel_id` FK to `channels.id`, `channels.default_type_id` FK
  to `channel_types.id`) — either matched by `descriptor.channel` or by the
  seeded user's `default_channel_id`. Grepping the entire `cbsd-server` crate
  for `INSERT INTO channels` / `INSERT INTO channel_types` turns up exactly one
  partial precedent (`db/robots.rs:1012`,
  `revive_resets_name_and_default_channel`), which seeds a bare `channels` row
  only — no `channel_types` row, and it never exercises `resolve_and_rewrite` at
  all. No test in this codebase today drives `resolve_and_rewrite` end-to-end
  against a real pool. `routes/periodic.rs`'s own existing test module
  (`can_manage_task` tests) is entirely pure-function; there is **no** existing
  handler-level test in `routes/periodic.rs` for `create_task`, `update_task`,
  or any other periodic handler to copy a fixture pattern from either
  (confirmed: the file's only `#[cfg(test)] mod tests` covers `can_manage_task`
  and one migration-SQL check, nothing that calls an `async fn ..._task`
  handler).

**Impact:** An implementer following the plan literally will write the "no body
→ 202" test, get an unexpected 400 (`UnknownComponent` or `EmptyComponents`),
and have to reverse-engineer — with no example to copy in this codebase — the
full chain (override `components`, insert a `channels` row, insert a matching
`channel_types` row, decide whether to set `descriptor.channel` explicitly or
the user's `default_channel_id`) before the test can even reach the code path it
exists to verify. This is exactly the kind of setup cost design v2's own review
flagged as a risk ("worth flagging so the eventual plan document doesn't
under-scope this test's setup cost") — and the plan does not do so. It also is
not reflected in the ~550 LOC estimate or the commit-2 file table, both of which
mention only the auth/token half of the setup (user, role, caps, bearer token).

**Recommendation:** Add a short paragraph to the plan's Commit 2 section naming
the fixture explicitly, e.g.: "the periodic task fixture used by the oneshot
202/400-priority tests needs (a) a `components` override on the test `AppState`
(struct-update over `test_app_state()`) matching the descriptor's component
name(s), and (b) a seeded `channels` + `channel_types` row pair (or an explicit
`descriptor.channel` avoiding the `default_channel_id` path)." Consider whether
this warrants a new `test_support.rs` helper (e.g.
`seed_resolvable_channel(pool) -> (channel_id, channel_type_id)`) since it is
generically useful and will likely be needed by any future handler test that
exercises `resolve_and_rewrite` — which, notably, has never been tested
end-to-end before this plan.

### S2 — Plan does not pin down when the priority-literal validation runs, which changes the fixture cost of one mandatory test

**Problem:** The design's numbered Flow lists "Effective priority" as step 8 —
after descriptor validation (3), tag interpolation (5), channel resolution (6),
and the robot guard (7) — but does not explicitly say whether the override
string's own validity (`"urgent"` → 400) is checked as a cheap, fail-fast step
immediately after body extraction, or only as part of step 8's late "compute
effective priority" logic. The plan inherits this ambiguity verbatim ("Priority:
manual match on the override ... fall back to the stored column ...") without
resolving it.

**Impact:** If validation happens at step 8 (literally per the design's step
order), the `{"priority": "urgent"} → 400` HTTP test needs the exact same full
fixture as the 202 cases (S1) to even reach that check — an invalid-descriptor
task would 400 for the wrong reason, giving a false-positive pass that doesn't
actually exercise the priority-match branch. If validation happens early (a
reasonable, arguably better, fail-fast implementation choice not precluded by
the design text), the 400 test needs no channel/component fixture at all and is
trivially cheap. This is a real fork in implementation cost and test correctness
that the plan should resolve, not inherit as ambiguity.

**Recommendation:** Pin the ordering explicitly in the plan (or during
implementation, in a code comment): either "the override literal is matched
immediately after body extraction, before step 1" (cheaper, more robust against
wasted work on trivially-bad input, and simplifies the 400 test to not need the
S1 fixture), or explicitly accept step-8-late validation and note that the 400
test therefore reuses the same fixture as the 202 tests. Either is defensible;
leaving it unstated is not.

### S3 — No shared test-support helper proposed for the bearer-token auth recipe used by all four mandatory oneshot tests

**Problem:** The plan's auth-setup recipe (seed user via
`create_or_update_user`, grant caps via `create_role` +
`set_role_caps_and_scopes` + `add_user_role`, mint a token via `token_create` +
`insert_token`, send `Authorization: Bearer`) is correct and fully verified
against the real function signatures, but it is described as being written
inline for each of the four HTTP-layer test cases. This is the **first**
fully-authenticated `oneshot`-based test in this codebase (the only existing
`oneshot` precedent, `rest_body_over_limit_returns_413` in `app.rs`, sends no
`Authorization` header at all — it tests the body-limit layer, which runs before
auth).

**Impact:** Four near-identical ~15-20 line setup blocks inlined in the same
test module is avoidable duplication, and — more importantly — this recipe will
almost certainly be needed again by any future optional-body or HTTP-layer-only
test in this codebase, since it is the only way to exercise `FromRequestParts`
for `AuthUser` through a real router.

**Recommendation:** Add a `test_support.rs` helper, e.g.
`pub async fn authed_bearer(pool: &SqlitePool, email: &str, caps: &[&str]) -> String`,
that performs the full seed-role-token-insert sequence and returns the raw
bearer token string. List `cbsd-server/src/routes/test_support.rs` in the Commit
2 file table if this is adopted.

## Minor Observations 🟢

- **~550 LOC estimate for Commit 2 is plausible but likely landing at the higher
  end of (or slightly past) that figure** once the S1 fixture code, the ~9-12
  direct-call tests enumerated in the design's Testing section, and the 4
  HTTP-layer tests (each with auth + fixture setup) are all written — plausibly
  650-750 authored lines. Still comfortably within the 400-800 guideline band,
  so not a split recommendation, just a heads-up so the number isn't treated as
  a hard ceiling that pressures under-testing.
- `db::tokens::insert_token`'s `expires_at: Option<i64>` parameter isn't
  mentioned in the plan's auth recipe (trivial — `None` is the obvious choice
  for a test token — but worth being explicit since it's the one parameter in
  the recipe not named).
- The design's own Testing section does not mandate unit-level coverage of the
  OCI-tag-invalid (400), channel/type-resolution-failure (400), or robot-guard
  (400) paths, and the plan correctly mirrors that scope exactly — not a plan
  deviation, just worth noting these error rows remain untested by design
  choice, not oversight, in case a future reader assumes otherwise.

## Strengths

- **Every cited symbol and signature checks out exactly.** A systematic pass
  through `db/periodic.rs`, `routes/periodic.rs`, `routes/builds.rs`,
  `scheduler/trigger.rs`, `channels/mod.rs`, `auth/extractors.rs`,
  `auth/paseto.rs`, `db/tokens.rs`, `db/roles.rs`, `db/users.rs`, and the `cbc`
  client/periodic/builds modules found zero incorrect claims about function
  existence, visibility, or signature — including the specific, checkable claim
  that `cbc::builds::parse_priority` is already `pub fn` and reachable from
  `cbc::periodic` (both modules are `pub mod` in `main.rs`; confirmed).
- **Commit boundaries pass the smell test and match this repository's own
  established precedent.** `git log` shows the exact same split shape
  (server-endpoint-and-scheduler commit, then a separate
  `cbc: add periodic build commands` commit) used for design 008's original
  implementation and again for the periodic short-id feature
  (`333c5f3f cbc: accept periodic task id prefixes` following server-side work).
  This is not a novel or risky split — it is the house style for this exact
  feature area, and each commit here delivers independently testable,
  non-dead-code functionality (curl-testable endpoint after commit 2; CLI parity
  after commit 3).
- **The README.md API-surface-table row is correctly placed in the server
  commit, not the docs commit.** The table is explicitly a "current-state
  index," and commit 1 (already landed as `83e6241`, predating any code) would
  misdocument an endpoint that doesn't exist yet if the row were added there.
  Landing it with the code that makes it true is the right call.
- **The HTTP-layer test mandate is grounded in this codebase's actual
  conventions, verified two ways.** First, the claim that direct-call handler
  tests are this codebase's norm is independently confirmed (`routes/admin.rs`'s
  `mod handler_tests` and `routes/auth.rs`'s test module both call handler
  functions directly with `auth_user(...)`). Second, the claim that
  `build_router(...).oneshot(...)` is workable precedent is confirmed against
  `app.rs`'s `rest_body_over_limit_returns_413`, and `build_router` was read in
  full: no rate-limiting, CSRF, or other middleware exists that would interfere
  with the four mandated cases beyond the already-accounted-for 1 MiB body
  limit.
- **The N1 fix from the design's v2 review propagated correctly.** The design's
  current OpenAPI code sample uses
  `request_body(content = Option<TriggerTaskBody>, ...)`, and the plan's utoipa
  annotation text matches it exactly — the fix was not silently dropped between
  design revision and plan authoring.
- **`cbc` LOC estimate (~180) is realistic to generous.** `cmd_enable`/
  `cmd_disable` in the same file are ~10 lines each with no request body;
  `cmd_trigger` needs a request/response DTO pair and a body-bearing POST, but
  following the existing `cmd_update`'s exact idiom for client-side priority
  validation (`parse_priority` called only to validate, discarding the parsed
  `Priority` and sending the raw string) keeps it well inside the estimate.

## Open Questions

1. Where exactly does the priority-override literal validation run in the
   implemented flow — immediately after body extraction, or as part of step 8's
   "effective priority" computation? (S2). This should be answered before the
   HTTP-layer 400 test is written, since it determines whether that test needs
   the S1 fixture.
2. Should the auth-setup recipe become a reusable `test_support.rs` helper (S3),
   given it is the first fully-authenticated `oneshot` test in this codebase and
   will likely be reused?
3. For the 202-path task fixture, will the descriptor set `channel` explicitly,
   or rely on the requester's `default_channel_id`? If the latter, the test will
   also visibly hit the known, already-documented S2-from-the-design-review
   `{channel}`-interpolates-to-empty-string behavior in the response's `tag`
   field — worth a one-line test comment noting this is expected, not a new bug,
   so a future reader doesn't flag it as a regression.

## Confidence Scoring

| Item                                                                                              | Points | Description                                                                                                                                                                                               |
| ------------------------------------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                                    | 100    |                                                                                                                                                                                                           |
| S1 — HTTP-layer test fixture strategy (components/channels) unspecified, no precedent in codebase | −15    | Deferred/incomplete plan work: a necessary implementation step (satisfying `validate_descriptor` and `resolve_and_rewrite` inside a `test_app_state()`-based test) is entirely unaddressed and unbudgeted |
| S2 — Priority-validation step ordering left ambiguous, affects test fixture cost                  | −5     | Plan-level spec under-specification inherited from the design's flow-step ordering, with a concrete, checkable downstream effect on what the mandatory 400 test requires                                  |
| S3 — No shared test-support helper proposed for the 4x-repeated auth recipe                       | −5     | Duplication risk: the same ~15-20 line setup sequence written inline four times within one commit, with no precedent helper to reuse and no proposal to create one                                        |
| **Total**                                                                                         | **75** |                                                                                                                                                                                                           |

Per the interpretation scale (75-89: "Acceptable with noted improvements. Fix
before next stage."), this plan is fundamentally sound — every code citation
verified correct, commit boundaries match established repository precedent
exactly, and design fidelity is otherwise complete — but should not proceed into
the commit-2 test-writing phase without first resolving S1/S2 (a short paragraph
or two pinning the fixture strategy and the priority-validation ordering). S3 is
a quality-of-life improvement, not a blocker.

## Findings Ordered by Severity

1. 🟡 **S1** — The plan's mandatory HTTP-layer 202 tests (and possibly the
   400-bad-priority test, see S2) cannot pass against `test_app_state()` as-is
   (`components: Vec::new()` fails descriptor validation) and require a
   resolvable `channels`/`channel_types` fixture that has zero precedent
   anywhere in this codebase; the plan neither names this cost nor budgets for
   it.
2. 🟡 **S2** — The plan does not state whether the priority-override literal is
   validated fail-fast (before any DB work) or late (design step 8, after
   descriptor/tag/channel processing); this directly determines whether the
   `{"priority": "urgent"} → 400` test needs the S1 fixture or can use a
   minimal, cheap task.
3. 🟡 **S3** — No shared `test_support.rs` helper is proposed for the
   bearer-token auth-setup recipe repeated across the four mandatory oneshot
   tests, despite this being the first fully-authenticated `oneshot` test in the
   codebase and a likely-reused pattern going forward.
4. 🟢 Minor — ~550 LOC estimate for Commit 2 is plausible but likely to land at
   the higher end of, or modestly past, that figure once S1's fixture code and
   the full test matrix are written; still within the 400-800 guideline band.
5. 🟢 Minor — `insert_token`'s `expires_at` parameter isn't named in the auth
   recipe (trivially `None` for tests).
6. 🟢 Minor — OCI-tag-invalid, channel-resolution-failure, and robot-guard paths
   remain untested at the unit level, consistent with (not a deviation from) the
   design's own Testing section scope.

## Verdict

**Go, with conditions.** The plan is a faithful, well-verified translation of
the approved design — no factual errors were found in any cited function,
signature, or code pattern, and the 3-commit structure matches this repository's
own established precedent for this exact feature area. Implementation may begin
on Commit 1 (already landed) and the bulk of Commit 2 (DB helper, handler, DTOs,
OpenAPI annotation, direct-call tests) without further plan changes. Before
writing the four mandatory HTTP-layer tests, resolve S1 and S2: state explicitly
how the test fixture satisfies `validate_descriptor` and `resolve_and_rewrite`
inside a `test_app_state()`- based `AppState`, and pin down where the
priority-literal validation runs in the implemented flow. S3 (a shared
auth-setup test helper) is a recommended improvement, not a condition.
