# 017 — Robot Accounts: Implementation Review v2

| Field    | Value                                                         |
| -------- | ------------------------------------------------------------- |
| Design   | `017-20260417T1130-robot-accounts.md` (v4)                    |
| Plan     | `017-20260419T2123-robot-accounts.md`                         |
| Scope    | Commits `766a1b7`, `f08b841`, `b1bd180`, `331cbae`, `8e7341b` |
|          | (P3 + R1 + R2 + R3 + post-autosquash fmt)                     |
| Branch   | `wip/cbsd-rs-robot-tokens`                                    |
| Base     | `main`                                                        |
| Reviewer | Opus 4.7                                                      |
| Date     | 2026-04-21                                                    |

## Scope Note

The plan progress table marks all eight commits "Done\*" with a note that the
fixup wave (autosquash) has already been applied to the targets. The delta since
the v1 impl review (`4d4cadd`, score 31/100) is:

- `b1bd180` — R2 (rotation, description, coexistence guards) **and** the bulk of
  the v1 fixup wave folded into R1 via autosquash (the R1 commit `5c43229` from
  v1's scope has been replaced by `f08b841` on this branch).
- `331cbae` — R3 (`cbc admin robots` subcommand tree).
- `8e7341b` — `cargo fmt` touch-up after the autosquash.

The v1 review was authored against an earlier SHA history; the fixup wave
consolidated the follow-up corrections into the original R1/R2 commits rather
than landing them as separate commits. This review therefore evaluates the state
at branch tip, cross-checking every v1 finding against the current code and
tests.

## Verification of v1 Findings

Every v1 finding was verified by reading the code (and tests where relevant) at
branch tip, not by trusting the plan's "Review v1 Follow-ups" section.

| v1 ID   | Severity  | Status at tip     | Evidence                                                                                                                                                                                                                                                     |
| ------- | --------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **F1**  | CRITICAL  | FIXED             | `BEGIN IMMEDIATE` on `create_or_revive` (db/robots.rs:512), `rotate_token` (:689), `tombstone_robot` (:594), raw-SQL on acquired `SqliteConnection`.                                                                                                         |
| **F2**  | CRITICAL  | FIXED             | `revive_robot_in_conn` (db/robots.rs:447-461) now rewrites `name`, clears `default_channel_id`, resets `created_at` and `updated_at`.                                                                                                                        |
| **F3**  | CRITICAL  | FIXED             | `create_or_revive_inner` (db/robots.rs:547) re-reads `users` row inside `BEGIN IMMEDIATE`; concurrent losers return `AlreadyActive` (→ 409).                                                                                                                 |
| **F4**  | CRITICAL  | PARTIAL           | 18 DB-layer tests in `db/robots.rs`, plus `routes/robots.rs::expires_tests`, `extractors::tests`, `db/users::tests`. Handler-level integration gap.                                                                                                          |
| **F5**  | CRITICAL  | FIXED             | Server-side `parse_iso_date_to_next_day_epoch` (routes/robots.rs:169) returns day-after midnight UTC; test confirms 2026-12-31 → 1798761600.                                                                                                                 |
| **F6**  | IMPORTANT | FIXED             | Inline `await` with warn-swallow at extractors.rs:254, :293, :315; no `tokio::spawn` in the auth path.                                                                                                                                                       |
| **F7**  | IMPORTANT | FIXED             | `create_or_revive_robot` (routes/robots.rs:232), `replace_entity_roles` (admin.rs:973), `add_entity_role` (admin.rs:1060).                                                                                                                                   |
| **F8**  | IMPORTANT | FIXED             | `create_or_update_user` (db/users.rs:64) rejects `robot:` prefix; wired at OAuth callback (routes/auth.rs:272-289) with unit test on db path.                                                                                                                |
| **F9**  | IMPORTANT | FIXED             | `activate_entity` returns `StatusCode::BAD_REQUEST` for tokenless robots (admin.rs:209).                                                                                                                                                                     |
| **F10** | IMPORTANT | FIXED             | `RobotDetail` carries `display_name`, `token_status { state, prefix, expires_at, first_used_at, last_used_at, token_created_at }`, `roles`, `effective_caps`.                                                                                                |
| **F11** | IMPORTANT | FIXED (design v4) | Wire is `serde_json::Value` matched as `Null \| String` (routes/robots.rs:161); absent field rejected by clap at the CLI and by `Option<Value>`-via-required `Deserialize` on server. Tested.                                                                |
| **F12** | IMPORTANT | FIXED             | `verify_hashed_token` + `generate_token_material` shared between API keys and robot tokens (token_cache.rs:232, :253); `verify_api_key` / `verify_robot_token` are ~10-line wrappers. `revoke_all_active_tokens` extracted as a shared DB helper.            |
| **F13** | MODERATE  | FIXED             | Both `create_robot_in_conn` (db/robots.rs:415) and `revive_robot_in_conn` (db/robots.rs:482) use `INSERT OR IGNORE` for user_roles.                                                                                                                          |
| **F14** | MODERATE  | FIXED (comment)   | Migration 007 carries the defensive-only comment on the `ON DELETE CASCADE` clause.                                                                                                                                                                          |
| **F15** | LOW       | UNFIXED (plan)    | `SetDefaultChannelBody` changed `i64` → `Option<i64>` still lives in commit `331cbae` (R3); commit message mentions the change but not as "breaking". Downgraded: this is a **compatible loosening** (old clients keep working), not a true breaking change. |
| **F16** | LOW       | FIXED (via F6)    | Inline await replaces the per-auth `pool.clone()` + spawn.                                                                                                                                                                                                   |
| **F17** | LOW       | N/A               | Plan sizing; not a code issue.                                                                                                                                                                                                                               |
| **F18** | LOW       | UNFIXED           | `is_unique_violation` at db/robots.rs:102 and admin.rs:764 still compares `"2067"` as a string. Minor, but now duplicated across two files.                                                                                                                  |

## New Findings

### G1 — IMPORTANT: display-identity wiring incomplete

The plan commit 6 mandates `display_identity()` "wired through every
actor-format call site (builds listing, build detail, audit logs)" **and** a
lint-style test that prevents regression.

Observed: robot-scoped handlers (`routes/admin.rs` for entity-op logs,
`routes/robots.rs` throughout) use `user.display_identity()` correctly. The rest
of the codebase still uses `user.email` on actor-format `tracing::info!` lines:

| File                                    | Line(s)                        | Scope                                                     |
| --------------------------------------- | ------------------------------ | --------------------------------------------------------- |
| `cbsd-server/src/routes/auth.rs`        | 417, 631                       | "user {} revoked their token", API-key revoke log         |
| `cbsd-server/src/routes/builds.rs`      | 165, 377, 394                  | "user {} submitted build", "user {} revoked queued build" |
| `cbsd-server/src/routes/permissions.rs` | 292, 443, 512                  | role create / update / delete actor logs                  |
| `cbsd-server/src/routes/channels.rs`    | 356, 386, 533, 585, 641        | channel / type audit logs                                 |
| `cbsd-server/src/routes/admin.rs`       | 504, 612, 752, 997, 1093, 1141 | worker-mgmt actor logs, role-assign actor logs            |
| `cbsd-server/src/routes/periodic.rs`    | 235, 439, 479, 521, 563        | periodic task audit logs                                  |

For most of these the actor is guaranteed to be a human (the acting caller needs
a cap like `permissions:manage` or `channels:manage` that robots cannot hold,
via the forbidden-cap strip). But two paths are substantive:

- `routes/builds.rs:165` — build submission. A robot can submit builds (that is
  the whole point of the feature). The audit line logs `user.email` (the
  synthetic form) instead of `robot:<name>`.
- `routes/builds.rs:377` — queued-build revoke. A robot with `builds:revoke:own`
  is legitimately allowed to revoke its own builds, and its audit line records
  `user.email`.

The missing **lint-style test** means any future addition of a handler using
`user.email` in an actor-format log line will not be caught. The plan calls this
out explicitly (plan § Tests commit 6, last bullet).

### G2 — IMPORTANT: HTTP handler integration tests absent

DB-layer coverage is strong: 18 tests in `db/robots.rs` exercise
create/revive/tombstone/rotate/standalone-revoke/set-description over the full
lifecycle, including the two concurrency tests
(`concurrent_revive_yields_exactly_one_winner`,
`concurrent_rotation_leaves_exactly_one_active_token`) that regression- guard
F1/F3. `validate_robot_name`, `display_identity`, the `${username}` predicate,
and `list_entities_filtered` are all unit- tested.

What is still missing: no test exercises a full HTTP request path through axum
for

- `POST /api/auth/api-keys` rejecting a robot caller (400 with hint),
- `POST /api/auth/tokens/revoke-all` rejecting a robot caller (400),
- `GET /api/admin/entities?type=garbage` returning 400 (not 500),
- `POST /api/builds` from a robot whose explicit-channel override uses
  `${username}` (the predicate unit test covers the pure function; the
  submit-build branch that calls it is not exercised),
- the 404 matrix across `/api/admin/robots/{name}/*` for unknown names (R2 plan
  mandate).

The plan names these specifically in commit 7's test list. The behaviour is
implemented and individually readable, but the regression-guard coverage stops
at the DB layer. Rating: IMPORTANT rather than CRITICAL because the behaviour is
observable by hand via the plan's manual smoke workflow, but a typical refactor
of the route could silently regress any of these checks.

### G3 — MODERATE: Argon2id verification skipped on no-prefix-match (timing channel)

`token_cache::verify_hashed_token` (token_cache.rs:285-288) returns
`TokenError::NotFound` immediately when `fetch_candidates` yields an empty
vector. This produces a measurable timing delta between

- "token prefix exists in DB" (~250 ms for Argon2id verify on default OWASP
  params), and
- "token prefix does not exist in DB" (~1 ms for the prefix lookup alone).

An attacker who can probe the bearer path can enumerate which 12-char hex
prefixes correspond to active tokens. They still cannot forge a token — the 52
remaining hex chars are 208 bits of secret entropy they do not observe — so the
exposure is limited to **prefix enumeration**, not credential leak.

The review brief explicitly flagged this: "Argon2id hash verification should run
even on non-existent tokens to avoid user-enumeration timing leaks — check this
is done." It is not done. Same behaviour exists on the `cbsk_` API-key path
(both share `verify_hashed_token`), so it is not a new regression — the
deduplication in F12 merely inherited the pre-existing pattern from
`verify_api_key`.

Fix: on empty candidate vec, run a dummy Argon2 verify against a static
known-bad hash under `spawn_blocking` before returning `NotFound`. Cheap, and
aligns with the brief's requirement.

### G4 — MODERATE: revive keeps revoked tokens rather than deleting them

Design § REST API step 3 (create-or-revive transaction):

> Delete all `robot_tokens` rows for this email (including revoked tombstones
> from the prior identity).

Implementation: `revive_robot_in_conn` (db/robots.rs:463) calls
`revoke_all_active_tokens_in_conn`, which UPDATEs non-revoked rows to
`revoked = 1`. It does not DELETE any row — the prior identity's revoked rows
remain as tombstones, and the revoke just adds another revoked row for the
token-that-was-active-at-tombstone.

Consequence: an admin inspecting `robot_tokens` for a revived robot sees token
rows from the prior identity alongside the new active one. Purely observational
— the active-token lookup filters on `revoked = 0`, so the stale rows cannot
authenticate. Arguably **more** auditable than the design's prescription (the
full token lineage is preserved). But it is a deliberate deviation from the
spec, and the design document should either be amended or the code should match
step 3 literally.

### G5 — LOW: tombstone ordering — insert before role delete in revive

`revive_robot_in_conn` (db/robots.rs:447-488) performs the steps in this order:
UPDATE users → revoke_all_active_tokens → **INSERT robot_tokens** → DELETE
user_roles → INSERT user_roles (with OR IGNORE).

The design presents the steps as: re-read → DELETE user_roles → DELETE
robot_tokens → UPDATE users → INSERT user_roles → INSERT robot_tokens. The
reordering is inside a single `BEGIN IMMEDIATE` transaction so it is atomic and
the observed end state is identical. Minor style remark; not a bug.

### G6 — LOW: `tombstone_robot` pre-read is outside the transaction

In `routes/robots.rs::tombstone_robot` (line 466), the robot row is loaded via
`get_robot_by_name(&state.pool, ...)` before the `BEGIN IMMEDIATE` transaction
opens inside the DB helper. The 404 branch is safe (no write). The race where a
concurrent revive reactivates the robot between the read and the tombstone is
benign — the tombstone is idempotent (set active=0, revoke any non-revoked
rows), so a revived-then-re-tombstoned state is self-consistent; the
revive-racer will see its work undone. Reader-level expectations should be
documented in the handler comment.

## Plan Coverage Matrix

Legend: `OK` implemented, `OK-test` implemented with test, `PART` partial,
`MISS` missing, `DEV` deviates.

### P3 — token usage tracking (commit `766a1b7`)

| Plan item                                              | Status                          |
| ------------------------------------------------------ | ------------------------------- |
| migration `006_token_usage.sql` adds columns           | OK                              |
| `validate_paseto` writes `last/first_used_at`          | OK (inline await, warn-swallow) |
| `verify_api_key` path writes `last/first_used_at`      | OK (inline await, warn-swallow) |
| `mark_used` helper in `db/tokens.rs`, `db/api_keys.rs` | OK                              |
| swallow-and-warn on update failure                     | OK                              |
| unit test for `mark_used` idempotence                  | MISS                            |
| unit test asserting `tracing::warn!` on failure        | MISS                            |

### R1 — robot provision + `cbrk_` auth (commit `f08b841`)

Schema:

| Plan item                                           | Status |
| --------------------------------------------------- | ------ |
| Migration `007_robot_accounts.sql` ALTER `users`    | OK     |
| `robot_tokens` table with partial unique index      | OK     |
| `idx_robot_tokens_prefix`, `idx_robot_tokens_robot` | OK     |

Permissions / hardening:

| Plan item                                                   | Status                                |
| ----------------------------------------------------------- | ------------------------------------- |
| `count_active_wildcard_holders` gains `AND u.is_robot = 0`  | OK (db/roles.rs:428)                  |
| `KNOWN_CAPS` gains `robots:manage`, `robots:view`           | OK (permissions.rs:36-37)             |
| Reject assignment of forbidden-cap roles to robots          | OK-test (admin.rs:965, :1060)         |
| Reject create/revive with role containing forbidden cap     | OK-test (robots.rs:232)               |
| SSO first-login rejects `users.name` starting with `robot:` | OK-test (db/users.rs:64, auth.rs:272) |

Entity handler extensions:

| Plan item                                                 | Status                         |
| --------------------------------------------------------- | ------------------------------ |
| `deactivate_entity` branches on `is_robot` (disable-only) | OK (admin.rs:131-137)          |
| Cache purged in both branches                             | OK (admin.rs:127)              |
| `activate_entity` rejects tokenless robot (400)           | OK (admin.rs:200-213)          |
| Cache purged on activate                                  | OK (admin.rs:230)              |
| `GET /api/admin/entities?type=user\|robot\|all`           | OK-test (admin.rs:844)         |
| `?type=garbage` returns 400 (plan item)                   | OK (handler path); MISS (test) |

Auth / cache generalisation:

| Plan item                                                    | Status                          |
| ------------------------------------------------------------ | ------------------------------- |
| `ApiKeyCache` → `TokenCache`; `CachedApiKey` → `CachedToken` | OK                              |
| `TokenKind { ApiKey, RobotToken }` discriminator             | OK                              |
| `AppState.api_key_cache` → `AppState.token_cache`            | OK                              |
| `cbrk_` bearer dispatch                                      | OK (extractors.rs:302)          |
| SHA-256 cache key, prefix lookup, Argon2id verify            | OK (token_cache.rs)             |
| `last_used_at` write on verified robot token                 | OK (extractors.rs:315)          |
| `expires_at IS NULL` → never expires                         | OK-test                         |
| Forbidden-cap strip in `load_authed_user`                    | OK-test (extractors.rs:197-199) |
| `AuthUser { is_robot }` field                                | OK                              |
| `AuthUser::display_identity()`                               | OK-test                         |
| `whoami` gains `is_robot`                                    | OK (auth.rs:370)                |
| Build response `is_robot` on submit/get/list                 | OK (builds.rs:61, :174)         |

Robot endpoints and CLI (server-side):

| Plan item                                                    | Status                                             |
| ------------------------------------------------------------ | -------------------------------------------------- |
| `POST /api/admin/robots` create / revive                     | OK-test (7 tests cover paths)                      |
| — `BEGIN IMMEDIATE` isolation                                | OK-test (`concurrent_revive_*`)                    |
| — re-read row under lock                                     | OK (create_or_revive_inner)                        |
| — revive resets `default_channel_id` / `created_at` / `name` | OK-test (`revive_resets_name_and_default_channel`) |
| — request body `expires: "YYYY-MM-DD" \| null`               | OK-test (4 `expires_tests`)                        |
| `GET /api/admin/robots` list                                 | OK (all design fields present)                     |
| `GET /api/admin/robots/{name}` details                       | OK (token_status, roles, effective_caps)           |
| `DELETE /api/admin/robots/{name}` tombstone                  | OK (BEGIN IMMEDIATE, 404 handling)                 |

Audit / display identity:

| Plan item                                                  | Status    |
| ---------------------------------------------------------- | --------- |
| `display_identity()` on actor-format calls in admin/robots | OK        |
| `display_identity()` on actor-format calls elsewhere       | MISS (G1) |
| Lint-style test preventing `user.email` in log macros      | MISS (G1) |

### R2 — token rotation + coexistence (commit `b1bd180`)

| Plan item                                                          | Status                                         |
| ------------------------------------------------------------------ | ---------------------------------------------- |
| `rotate_token(tx, ...)` DB helper                                  | OK-test                                        |
| `POST /api/admin/robots/{name}/token` with `renew` body flag       | OK-test (`rotate_*` in db tests)               |
| — `BEGIN IMMEDIATE` on the rotation transaction                    | OK-test (`concurrent_rotation_*`)              |
| — wire format: `"YYYY-MM-DD" \| null`                              | OK-test                                        |
| `DELETE /api/admin/robots/{name}/token` — idempotent 200           | OK-test (`standalone_revoke_is_idempotent`)    |
| — 404 on unknown name                                              | OK (route-level; MISS handler test)            |
| `PUT /api/admin/robots/{name}/description`                         | OK-test (`set_description_updates_and_clears`) |
| — 404 on unknown name                                              | OK-test (in `set_description_*`)               |
| `list_entities_filtered` + `?type=user\|robot\|all`                | OK-test (db/users::tests)                      |
| — 400 on invalid filter value                                      | OK (route path); MISS test                     |
| `prefix_template_contains_username` predicate                      | OK-test                                        |
| `set_entity_default_channel` rejects robot + `${username}` channel | OK (admin.rs:711); MISS test                   |
| `submit_build` rejects robot + `${username}` resolved channel      | OK (builds.rs:133); MISS test                  |
| `POST /api/auth/api-keys` rejects robot caller                     | OK (auth.rs:513); MISS test                    |
| `POST /api/auth/tokens/revoke-all` rejects robot caller            | OK (auth.rs:431); MISS test                    |
| R2 handler-level tests (rotation, 404, coexistence)                | PART (DB-layer only)                           |

### R3 — `cbc admin robots` (commit `331cbae`)

| Plan item                                                                             | Status                                            |
| ------------------------------------------------------------------------------------- | ------------------------------------------------- |
| `cbc admin robots create` with `--expires`, `--no-expires`, `--description`, `--role` | OK-test                                           |
| — date-or-`--no-expires` enforced at clap layer                                       | OK (`required_unless_present` + `conflicts_with`) |
| — help text states UTC semantics                                                      | OK (admin/robots.rs:69-71)                        |
| `cbc admin robots list`                                                               | OK                                                |
| `cbc admin robots get` (token status, roles, effective caps)                          | OK                                                |
| `cbc admin robots set-description`                                                    | OK                                                |
| `cbc admin robots enable` / `disable`                                                 | OK                                                |
| `cbc admin robots roles set/add/remove`                                               | OK                                                |
| `cbc admin robots default-channel set/clear`                                          | OK                                                |
| `cbc admin robots token new [--renew]`                                                | OK-test                                           |
| `cbc admin robots token revoke --yes-i-really-mean-it`                                | OK                                                |
| `cbc admin robots delete --yes-i-really-mean-it`                                      | OK                                                |
| `cbc admin users list` appends `?type=user`                                           | OK                                                |
| `cbc admin users get` rejects robot email                                             | OK                                                |
| Manual smoke workflow from plan                                                       | Not executed (plan describes it as manual)        |

## Commit Granularity Evaluation

Applying the `git-commits` skill (400-800 authored LOC target):

| #   | Commit       | Authored LOC                                               | Compiles alone | Meaningful alone                                                            | Verdict                                                               |
| --- | ------------ | ---------------------------------------------------------- | -------------- | --------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| 5   | `766a1b7` P3 | ~83                                                        | yes            | symmetric tracking across token types                                       | **PASS** (below target but single concern)                            |
| 6   | `f08b841` R1 | ~2800 (incl. large rename of api_keys.rs → token_cache.rs) | yes            | robot provision + auth end-to-end including the TokenCache rename folded in | **MARGINAL** (above upper bound; size dominated by the module rename) |
| 7   | `b1bd180` R2 | ~1100                                                      | yes            | rotation + coexistence + `?type` filter                                     | **MARGINAL** (above target; rotation + coexistence could have split)  |
| 8   | `331cbae` R3 | ~1070                                                      | yes            | cbc subcommand tree + server default-channel null                           | **MARGINAL** (above target; F15 bundling)                             |

All commits compile alone; the workspace builds cleanly at every ancestor
checkout (`SQLX_OFFLINE=true cargo check --workspace`). Commit messages use the
`component: description` prefix convention and all carry DCO sign-off + the
`Co-authored-by:` trailer required by `cbsd-rs/CLAUDE.md`.

The size bloat in commits 6-8 is partly real (robot provisioning is a large
vertical feature) and partly a consequence of folding v1-fixup commits into
their targets via autosquash. The autosquash was the right call — the
intermediate `--fixup` commits would have been meaningless on their own — but
the targets are now bigger than the original plan estimates suggested.

## Confidence Score

Starting at 100, applying the `confidence-scoring` deduction table:

| Deduction                    | Finding(s)                                                         | Points  |
| ---------------------------- | ------------------------------------------------------------------ | ------- |
| D2 Missed requirement (spec) | G1 display-identity incomplete (plan-mandated wiring + lint test)  | −5      |
| D2 Missed requirement (spec) | G4 revive revokes rather than deletes prior tokens (design step 3) | −3      |
| D3 Correctness bug           | G3 Argon2 skip on no-prefix-match (timing channel named by brief)  | −5      |
| D6 Test coverage             | G2 handler-level integration tests absent (plan-mandated)          | −6      |
| D13 Minor bug / style        | F18 string "2067" compare now duplicated across two files          | −1      |
| D13 Minor bug / style        | G5 revive step ordering differs from spec (no behaviour change)    | −1      |
| D13 Minor bug / style        | F15 R3 commit bundled default-channel null; plan not amended       | −1      |
| **Total**                    |                                                                    | **−22** |

**Score: 78 / 100**

Rationale for the delta from v1 (31 → 78): every v1 CRITICAL finding (F1-F5) is
resolved with code and regression tests. The concurrency correctness (F1/F3) is
actively tested via two `tokio::spawn`-based race tests
(`concurrent_revive_yields_exactly_one_winner`,
`concurrent_rotation_leaves_exactly_one_active_token`) which fail if
`BEGIN IMMEDIATE` is removed or the re-read-under-lock is dropped. The
off-by-one-day bug (F5) is fixed server-side with explicit unit-test anchors
(`date_string_yields_next_day_midnight_utc_epoch`). The design- contract shape
(F10/F11) now matches design v4 byte-for-byte.

Remaining deductions are either newly surfaced (G3 timing channel — not a
regression, pre-existing behaviour inherited via F12 dedup), minor scope-creep
(G1 partial wiring + missing lint test), or test-coverage at the handler vs DB
layer (G2). None are user-visible production bugs.

## Go / No-Go Recommendation

**Go** — with the following follow-ups tracked before the branch merges to
`main`. None are blockers.

### Follow-ups (ordered by priority)

1. **G3** — add a dummy Argon2 verify on empty-candidate path in
   `verify_hashed_token`. Keeps prefix enumeration symmetrically timing-matched
   with a real verify. Applies to both `cbsk_` and `cbrk_` paths since they
   share the code.
2. **G1a** — switch remaining `user.email` → `user.display_identity()` on
   `routes/builds.rs:165` (submit) and `routes/builds.rs:377` (revoke queued) at
   minimum. Other locations (admin worker management, permissions mutation,
   channel audit) have robots-cannot-reach-here guarantees via forbidden-cap
   strip and can be migrated opportunistically.
3. **G1b** — add the lint-style test (plan § Tests commit 6). Walk
   `cbsd-server/src/routes/*.rs`, grep for `tracing::(info|warn|error)!`
   containing `user.email` outside an allowlist, fail the test on matches.
   Regression guard only — no current bug.
4. **G2** — add handler-level tests for the four coexistence-guard branches
   (`/auth/api-keys` robot target, `/auth/tokens/revoke-all` robot caller,
   `/admin/entities?type=garbage`, `/builds` robot + `${username}` channel) plus
   the `/admin/robots/{name}/*` 404 matrix. Requires an axum test harness; worth
   the investment since the business-logic tests are already in place and this
   closes the regression gap.
5. **G4** — either amend the design document to describe the observed
   revoke-not-delete behaviour as the intended revive semantics, or change the
   code to `DELETE FROM robot_tokens WHERE robot_email = ?` before the INSERT.
   The design's "builds.user_email attribution must survive" invariant is
   satisfied by either.
6. **F18** — extract `is_unique_violation` into a single shared helper (e.g.
   `crate::db::sqlite_is_unique_violation`) and remove the duplicate in
   `routes/admin.rs`. Optional: match on `sqlx::Error`'s typed
   `ErrorKind::Database` path rather than stringly on `"2067"`. Cosmetic.

### Nice-to-have (no blocker)

7. Add P3 unit tests for `mark_used` idempotence and warn-on-failure.
8. Amend the plan to reflect the autosquash (R1 is now `f08b841`, not
   `5c43229`).
9. Consider splitting the plan's "commit 6" on future features that have a
   similar "TokenCache rename + feature" shape — the rename inflated LOC more
   than the robot feature itself.

## Summary of Top Findings (Severity-Ordered)

1. **G3 — MODERATE** — Argon2 verification is skipped when no prefix candidates
   exist, enabling prefix-enumeration via timing. Named by the review brief.
   Pre-existing on the API-key path; inherited by the robot path via the F12
   deduplication. Fix is a ~10-line dummy- verify insertion in one function.

2. **G2 — IMPORTANT** — Handler-level integration tests are absent for the
   coexistence guards, the `?type=` filter's 400 branch, the `${username}`
   reject on build submit, and the 404 matrix on `/admin/robots/{name}/*`.
   DB-layer tests cover the business logic thoroughly (18 tests in
   `db/robots.rs`, plus users/extractors/ channels unit tests); the gap is at
   the axum-router layer.

3. **G1 — IMPORTANT** — Plan's "display_identity() wired through every
   actor-format call site" is partially done; several robot-reachable paths
   (notably `submit_build` and `revoke_build`) still log `user.email`. The
   plan's lint-style test that would catch this is also missing.

4. **G4 — MODERATE** — Revive revokes rather than deletes prior `robot_tokens`
   rows, a spec deviation. Arguably more auditable; needs either a design
   amendment or a one-line code fix.

5. **F18 — LOW** — `is_unique_violation` uses string "2067" comparison and is
   now duplicated between `db/robots.rs:102` and `routes/admin.rs:764`.
   Consolidate into a shared helper.

6. **G5, G6, F15** — cosmetic: revive step ordering, tombstone pre- read outside
   transaction, R3 commit bundled a compatible-loosening server change without
   amending the plan.

**Confidence score: 78/100.** Recommendation: **go**, with the follow-ups above
tracked. The feature is production-ready for robot account provisioning and
authentication; the remaining items are hardening and completeness.
