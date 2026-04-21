# 017 — Robot Accounts: Implementation Review v3

| Field    | Value                                                                                      |
| -------- | ------------------------------------------------------------------------------------------ |
| Design   | `017-20260417T1130-robot-accounts.md` (v4)                                                 |
| Plan     | `017-20260419T2123-robot-accounts.md`                                                      |
| Scope    | Commits `5d51cf9 .. 12ae11f` (12 commits — P1, P2-guard, P2, P3, R1, R2, R3, docs, v2 fmt) |
| Branch   | `wip/cbsd-rs-robot-tokens` at `12ae11f`                                                    |
| Base     | `main`                                                                                     |
| Reviewer | Opus 4.7                                                                                   |
| Date     | 2026-04-21                                                                                 |

## Scope Note

The scope runs from `5d51cf9` (P1) through `12ae11f` (v2 review doc).
Enumerated:

| SHA       | Subject                                                            |
| --------- | ------------------------------------------------------------------ |
| `5d51cf9` | cbc: require --yes-i-really-mean-it on irreversible commands (P1)  |
| `ff349b3` | cbsd-rs/server: deduplicate last-admin guard into shared db helper |
| `be5afca` | cbsd-rs: reshape account-level admin endpoints under /admin/entity |
| `8325bb8` | cbsd-rs: track first_used_at / last_used_at on tokens and API keys |
| `6335e7d` | cbsd-rs: robot account provision and cbrk\_ bearer auth (R1)       |
| `e842049` | cbsd-rs/docs: mark R1 done in plan progress table                  |
| `cc9d331` | cbsd-rs/server: add robot token lifecycle and coexistence guards   |
| `618cad4` | cbc: add admin robots subcommand tree                              |
| `d216037` | cbsd-rs/docs: add implementation review for robot accounts         |
| `ef4b897` | cbsd-rs/docs: plan progress + review v1 follow-ups section         |
| `ee0d23e` | cbsd-rs/server: cargo fmt post-autosquash                          |
| `12ae11f` | cbsd-rs/docs: add v2 implementation review for robot accounts      |

The v2 review scored the previous revision of this branch at 78/100 with six new
findings (G1–G6) and one remaining v1 carry-over (F18). Between v2 and v3 the
user applied a fixup wave that autosquashed into the phase commits, replacing
`766a1b7/f08b841/b1bd180/331cbae` with `8325bb8/6335e7d/cc9d331/618cad4`. This
review evaluates the post-autosquash state at branch tip. Every v2 follow-up
claim was cross-checked against the actual code, not the plan's self-report.

Build + tests at branch tip:

```text
SQLX_OFFLINE=true cargo check --workspace --all-targets   # OK
SQLX_OFFLINE=true cargo test   --workspace                 # 102 server tests,
                                                            # 12 worker tests,
                                                            # 0 failed
```

## Verification of v2 Follow-ups

| v2 Finding      | Claim                                                                  | Status at tip | Evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| --------------- | ---------------------------------------------------------------------- | ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F18             | `is_unique_violation` extracted to shared `db::is_unique_violation`    | CLOSED        | `db/mod.rs:65`; `db/robots.rs:91,668` use `super::is_unique_violation`; `routes/admin.rs:375,392` use `db::is_unique_violation`. `routes/channels.rs:652` still has a local copy but that pre-dates this phase (plan § Review v2 Follow-ups explicitly keeps it out of scope).                                                                                                                                                                                                               |
| G3              | Argon2id dummy verify on empty-candidate path                          | CLOSED        | `auth/token_cache.rs:42-45` defines `DUMMY_ARGON2_HASH` as a `LazyLock`; `:300-311` runs `argon2_verify(raw, &DUMMY_ARGON2_HASH)` in `spawn_blocking` before returning `NotFound`. Regression test `dummy_argon2_hash_initialises_and_rejects_non_sentinel_inputs` (:469-482) parses the sentinel as a valid PHC string and asserts a non-sentinel plaintext is rejected.                                                                                                                    |
| G4              | Revive DELETEs prior `robot_tokens` rows                               | CLOSED        | `db/robots.rs:454-456` calls `DELETE FROM robot_tokens WHERE robot_email = ?` in `revive_robot_in_conn`. Regression test `revive_deletes_prior_robot_tokens_rows` (:1358-1396) seeds a tombstoned row, revives, asserts `rows.len() == 1` and the only row is the freshly issued one.                                                                                                                                                                                                        |
| G5              | Revive step ordering matches design § REST-API step 3                  | CLOSED        | `db/robots.rs:450-493` executes DELETE user_roles → DELETE robot_tokens → UPDATE users → INSERT user_roles → INSERT robot_tokens. Comment at :439-447 cites the design section.                                                                                                                                                                                                                                                                                                              |
| G1a             | `user.display_identity()` on `routes/builds.rs` submit + revoke lines  | CLOSED        | `routes/builds.rs:165` (submit_build), `:377` (revoke_build queued branch), `:394` (revoke_build dispatched branch) all use `user.display_identity()`.                                                                                                                                                                                                                                                                                                                                       |
| G1b             | Audit-identity lint test with allowlist + self-tests                   | CLOSED        | `routes/audit_identity_lint.rs` (169 lines). Scans every `.rs` under `routes/` except `HUMAN_ONLY_ROUTES` or `EXEMPT_ROUTES`; flags any `tracing::(info\|warn\|error)!` call whose argument list contains `user.email`. Three self-tests (single-line, non-log ignore, multi-line macro). Test passes at HEAD.                                                                                                                                                                               |
| G6              | `tombstone_robot` pre-read race documented in handler                  | CLOSED        | `routes/robots.rs:455-462` documents the benign race: 404 branch is write-free; concurrent revive that reactivates mid-call is undone idempotently by the tombstone.                                                                                                                                                                                                                                                                                                                         |
| G2 (R1)         | Handler tests for `get_robot` and `tombstone_robot` (404 + 403 matrix) | CLOSED        | `routes/test_support.rs` provides `test_pool`, `test_app_state`, `auth_user`. `routes/robots.rs::handler_tests` has 7 handler tests total — `get_robot_of_unknown_name_returns_404`, `tombstone_robot_of_unknown_name_returns_404`, `get_robot_without_view_cap_returns_403`, `tombstone_robot_without_manage_cap_returns_403`, plus the three R2 404 tests below.                                                                                                                           |
| G2 (R2)         | Seven handler tests for coexistence + 404 matrix                       | CLOSED        | Counted: `routes/robots.rs` — `rotate_token_of_unknown_robot_returns_404`, `revoke_token_of_unknown_robot_returns_404`, `set_description_of_unknown_robot_returns_404` (3); `routes/auth.rs::handler_tests` — `create_api_key_rejects_robot_caller_with_400`, `revoke_all_tokens_rejects_robot_caller_with_400` (2); `routes/admin.rs::handler_tests` — `list_entities_rejects_unknown_type_filter_with_400`, `list_entities_accepts_absent_type_filter_as_all` (2). Total 7, matches claim. |
| P3 nice-to-have | `mark_*_used` idempotence tests across all three token types           | CLOSED        | `db/tokens.rs:132` `mark_token_used_preserves_first_used_at_across_calls`; `db/api_keys.rs:258` `mark_api_key_used_preserves_first_used_at_across_calls`; `db/robots.rs:1399` `mark_robot_token_used_preserves_first_used_at_across_calls`. Each zeros `last_used_at`, re-calls, asserts `first_used_at` preserved via COALESCE and `last_used_at` overwritten. Each file also has `mark_*_used_skips_revoked_rows`.                                                                         |
| Plan amends     | v2 follow-ups section + SHA refresh + F15 note                         | CLOSED        | Plan document §§ "Review v2 Follow-ups" (p.884), "Autosquash SHA refresh" (p.858), "R3 bundled change (F15)" (p.874) all present at branch tip.                                                                                                                                                                                                                                                                                                                                              |
| v2 doc          | Standalone commit                                                      | CLOSED        | `12ae11f` is a docs-only commit adding `017-20260421T1704-impl-robot-accounts-v2.md`.                                                                                                                                                                                                                                                                                                                                                                                                        |

Every claim in the brief's checklist was confirmed by reading the code.

## New Findings

### G7 — MODERATE: `POST /api/auth/token/revoke` silently no-ops for robot callers

`routes/auth.rs::revoke_token` (line 381) is the self-revoke endpoint. It takes
`AuthUser` from the extractor, which accepts `cbrk_` robot tokens. The handler
prologue rejects `cbsk_` bearers with a 400 pointing at
`DELETE /api/auth/api-keys/:prefix`:

```rust
if auth_header.starts_with("cbsk_") {
    return Err(auth_error(
        StatusCode::BAD_REQUEST,
        "use DELETE /api/auth/api-keys/:prefix to revoke API keys",
    ));
}
```

There is no equivalent check for `cbrk_`. A robot-authenticated caller proceeds
past this guard and:

1. `paseto::token_hash(auth_header)` hashes the raw `cbrk_` string (line 409) as
   though it were PASETO.
2. `db::tokens::revoke_token(&state.pool, &hash)` (line 410) UPDATEs the
   `tokens` table filtering by that hash; robot tokens live in `robot_tokens` so
   this is a zero-row UPDATE.
3. `tracing::info!("user {} revoked their token", user.email)` (line 417) logs
   the synthetic `robot+<name>@robots` email — a G1a-style leak the lint test
   does not catch because `auth.rs` is on the `HUMAN_ONLY_ROUTES` allowlist (see
   G8 below on why the allowlist's rationale is wrong).
4. Returns 200 with body `{"detail": "token revoked"}` — misleading, because the
   robot's `cbrk_` token continues to authenticate successfully.

Why this is MODERATE not CRITICAL: the robot token is not actually revoked, so
no credential is lost. The production blast radius is operator confusion (a CI
system that "revoked itself" but keeps making authenticated calls). But it is a
latent footgun — an operator running the cbc client as an automation caller
would see success and interpret it as defense-in-depth.

Fix: mirror the `cbsk_` branch. Prepend

```rust
if auth_header.starts_with("cbrk_") {
    return Err(auth_error(
        StatusCode::BAD_REQUEST,
        "robot accounts cannot self-revoke — use \
         DELETE /api/admin/robots/{name}/token",
    ));
}
```

This is consistent with the two other coexistence guards at `:431` and `:513`
(each rejects a robot caller with a pointer to the admin endpoint they should
use).

Regression coverage: a handler test mirroring
`create_api_key_rejects_robot_ caller_with_400` but invoking `revoke_token` with
a robot `AuthUser` needs a bearer header in `HeaderMap`; the existing test
harness builds the `AuthUser` directly but not the headers. Adding
`revoke_token`'s test shape is slightly more involved than the existing
coexistence tests (needs a `HeaderMap` with `Authorization: Bearer cbrk_...`),
but remains cheap.

### G8 — LOW: `HUMAN_ONLY_ROUTES` allowlist rationale mis-states robot reachability

`routes/audit_identity_lint.rs:29-38` lists `auth.rs` on the `HUMAN_ONLY_ROUTES`
allowlist with the rationale _"robots can't authenticate via SSO, API-key, or
session"_.

This is true for three of the handlers in the file (`logout`, `oauth_callback`,
`create_api_key_handler`), but `revoke_token` is reachable by robots because the
extractor accepts `cbrk_` bearers. The rationale should be narrowed — or,
equivalently, the broken reachability should be fixed by G7 (after which the
allowlist rationale becomes true again, since a robot caller would be rejected
before reaching a `user.email` log line).

Related: `admin.rs` is on the allowlist with the rationale _"admin:\* caps —
robots cannot hold"_, but `list_entities` (:827) requires only
`permissions:view`, which is _not_ in `ROBOT_FORBIDDEN_CAPS`. A future role
containing only `permissions:view` would let a robot reach `list_entities` and
any log line in that handler would escape the lint. No current bug
(`list_entities` does not log `user.email` in a tracing macro), but the
allowlist is over-broad as a regression guard.

Fix options, in increasing scope:

1. Narrow the `auth.rs` rationale to exclude `revoke_token` once G7 is closed.
2. Replace file-level allowlisting with function-level annotations (e.g. a
   `// audit-lint: human-only` marker grep'd by the test) so
   partially-robot-reachable files can be lint-enforced.
3. Drop the allowlist entirely and migrate every `user.email` log line in
   `admin.rs`, `permissions.rs`, `periodic.rs`, `channels.rs` to
   `display_identity()`. Cost: ~20 line changes, all currently human-only so no
   behaviour change.

Option 1 is the minimum fix and aligns with G7. Option 3 is the plan's stated
intent (plan § Tests commit 6, last bullet: _"Prevents regression when new
handlers are added"_); a future phase could pick it up.

### G9 — LOW: R1 commit `6335e7d` breaks `cargo check --all-targets`

Verified by checking out `6335e7d` in a scratch worktree:

```text
$ SQLX_OFFLINE=true cargo check --workspace             # OK
$ SQLX_OFFLINE=true cargo check --workspace --all-targets
error: `SQLX_OFFLINE=true` but there is no cached data for this query
error[E0425]: cannot find function `prefix_template_contains_username`
  (x 11 occurrences in cbsd-server/src/channels/mod.rs)
error[E0282]: type annotations needed
...
error: could not compile `cbsd-server` (bin "cbsd-server" test) due to
       13 previous errors
```

Root cause: R1 adds `#[cfg(test)] mod username_predicate_tests` in
`cbsd-server/src/channels/mod.rs`, but the target function
`prefix_template_contains_username` lives in R2 (`cc9d331`).

Consequences:

- `git bisect` across this phase that runs `cargo check --all-targets` or
  `cargo test` bisects incorrectly: R1 is flagged as broken when the R1 feature
  itself is fine.
- CI / review workflows that run the full test target at every commit fail at
  R1.

Non-consequences: the project's documented pre-commit sequence
(`cbsd-rs/CLAUDE.md`: `cargo fmt → cargo clippy → cargo check --workspace`) does
not include `--all-targets`, so day-to-day development and the R1 commit's own
validation passed the gate. The defect is only visible to tooling that runs
tests at every ancestor commit.

This was a pre-existing issue before the v2 wave (both impl reviews noted it in
passing). The v2 fixup wave did not touch it because the fix is structural:
either fold the test into R2, or split R1 to land the test alongside the
function it calls. Neither is appropriate as a post-hoc fixup.

Severity: LOW. The phase is not structurally broken and the authored code at R1
is correct; only the test module is mis-ordered. The bisectability penalty is
small because the next commit after R1 on this branch is a docs-only commit
(`e842049`), and R2 (`cc9d331`) immediately resolves it.

Recommendation: accept and document in the plan's `Autosquash SHA refresh` table
so future bisect sessions know to skip past R1's test target. Alternatively,
fold the `username_predicate_tests` module into R2 via a second autosquash; the
post-squash diff is mechanical (one `#[cfg(test)] mod` moves between two
`channels/mod.rs` revisions).

## Remaining Verifications

| Concern from the brief                                    | Verdict at tip        | Evidence                                                                                                                                                                                                                                                                                                                                                                                   |
| --------------------------------------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `cbrk_` bearer path, Argon2id timing-parity               | OK                    | `auth/token_cache.rs:300-311` runs dummy verify; regression test asserts sentinel rejects non-sentinel input.                                                                                                                                                                                                                                                                              |
| Tombstone + revive semantics, `builds.user_email` history | OK                    | `tombstone_robot_inner` (db/robots.rs:613) sets `active = 0` and revokes tokens but keeps the `users` row; `revive_robot_in_conn` rewrites `name`, `robot_description`, `default_channel_id`, `created_at`, `updated_at`; tests `revive_resets_name_and_default_channel` and `revive_deletes_prior_robot_tokens_rows` cover both.                                                          |
| Coexistence guards: email namespace                       | OK                    | `db/users.rs:64` (`create_or_update_user`) rejects `name` starting with `robot:`; wired into OAuth callback `routes/auth.rs:272-289`; unit test on DB path.                                                                                                                                                                                                                                |
| Coexistence: API key + PASETO revoke-all reject           | PARTIAL (see G7)      | `create_api_key_handler` rejects `is_robot` before cap check (:513-518). `revoke_all_tokens` rejects before cap check (:431-436). **`revoke_token` (self-revoke) does NOT reject `cbrk_`** — G7.                                                                                                                                                                                           |
| Coexistence: `${username}` channel reject                 | OK                    | `routes/builds.rs:133-140` rejects on resolved channel; `routes/admin.rs:711-732` rejects at default-channel assignment. `channels::prefix_template_contains_username` unit tests at `channels/mod.rs::username_predicate_tests`.                                                                                                                                                          |
| `BEGIN IMMEDIATE` on create_or_revive, rotate, tombstone  | OK                    | `db/robots.rs:517` (create_or_revive), `:599` (tombstone_robot), `:694` (rotate_token). All three also re-read the `users` row under the reserved lock before writing. Concurrency regression tests `concurrent_revive_yields_exactly_one_winner` and `concurrent_rotation_leaves_exactly_one_active_token` present.                                                                       |
| Forbidden-cap strip in `load_authed_user`                 | OK                    | `auth/extractors.rs:197-199` retains only safe caps when `user.is_robot`. `ROBOT_FORBIDDEN_CAPS` = `[*, permissions:manage, robots:manage, apikeys:create:own]`.                                                                                                                                                                                                                           |
| Assignment-time reject paths                              | OK                    | `routes/robots.rs:232-237` (create/revive); `routes/admin.rs:965,1060` (replace_entity_roles + add_entity_role); all call `first_robot_forbidden_cap`.                                                                                                                                                                                                                                     |
| `display_identity()` on robot-reachable audit logs        | OK (modulo G7)        | `routes/builds.rs` (submit, revoke) and `routes/robots.rs` (throughout) use `display_identity()`. `admin.rs` entity handlers use `display_identity()` too (optional, but consistent). G7 is the one gap the lint does not catch.                                                                                                                                                           |
| `target` vs `actor` on `revoke_all_tokens` log line       | Acceptable            | `routes/auth.rs:464` logs `"revoked {count} tokens for user {}", body.user_email` — the _target_ of the revoke, not the actor. Actor is the caller (already rejected upfront if robot); target is a human. Logging the target here is informational, not an actor-identity leak.                                                                                                           |
| Coexistence check order (before or after cap check)       | OK                    | `create_api_key_handler` and `revoke_all_tokens` both check `user.is_robot` before `user.has_cap(...)`. A robot missing the cap is still rejected with 400 (coexistence), not 403, which is the correct semantics. Handler tests confirm by exercising robots _with_ the cap granted.                                                                                                      |
| `audit_identity_lint` self-test coverage                  | OK (with G8 caveat)   | Three self-tests cover: single-line macro, non-macro ignore, multi-line macro. String literals containing `"user.email"` inside other functions are not tested, but the heuristic's false-positive surface (any literal containing that substring inside a `tracing::` macro body) is acceptable given the zero-tolerance policy. G8 is about the allowlist scope, not the scanner itself. |
| P3 warn-on-failure test                                   | Deferred (documented) | Plan § Review v2 Follow-ups (p.902-908) documents the rationale: the inline warn-and-swallow pattern is not extracted into a helper, so testing the warn emission requires either `tracing-test` or a pattern refactor. The _behaviour_ is exercised at every auth-path request; only automated regression coverage of the warn line is absent. Acceptable deferral.                       |

## Plan Coverage Matrix

Legend: `OK` implemented, `OK-test` implemented with test, `PART` partial,
`MISS` missing, `DEV` deviates, `DEFER` documented deferral.

### P1 — `--yes-i-really-mean-it` (commit `5d51cf9`)

| Plan item                                                     | Status |
| ------------------------------------------------------------- | ------ |
| `cbc admin roles delete` renamed `--force` → flag             | OK     |
| `cbc admin workers deregister` gains flag                     | OK     |
| `cbc periodic delete` gains flag                              | OK     |
| Server `DELETE /api/permissions/roles/{name}` always cascades | OK     |

### P2-guard — consolidate last-admin guard (commit `ff349b3`)

| Plan item                                                       | Status |
| --------------------------------------------------------------- | ------ |
| Inline `SELECT COUNT(...)` in `deactivate_user` replaced        | OK     |
| `count_active_wildcard_holders_tx(&mut SqliteConnection)` added | OK     |
| `.sqlx/` regenerated                                            | OK     |

### P2 — `/api/admin/entity` reshape (commit `be5afca`)

| Plan item                               | Status |
| --------------------------------------- | ------ |
| 8 endpoint migrations (old → new paths) | OK     |
| `GET /api/admin/entities` added         | OK     |
| Router mount updated in `routes/mod.rs` | OK     |
| `cbc` URL literals updated              | OK     |
| Tests updated for new URL shape         | OK     |

### P3 — token usage tracking (commit `8325bb8`)

| Plan item                                               | Status  |
| ------------------------------------------------------- | ------- |
| migration `006_token_usage.sql`                         | OK      |
| `validate_paseto` writes `first/last_used_at` (inline)  | OK      |
| `verify_api_key` writes (inline)                        | OK      |
| `mark_used` helpers in `db/tokens.rs`, `db/api_keys.rs` | OK      |
| swallow-and-warn on failure                             | OK      |
| `mark_*_used` idempotence tests                         | OK-test |
| `tracing::warn!` emission test                          | DEFER   |

### R1 — robot provision + `cbrk_` auth (commit `6335e7d`)

Schema + permissions:

| Plan item                                                   | Status  |
| ----------------------------------------------------------- | ------- |
| Migration `007_robot_accounts.sql`                          | OK      |
| `robot_tokens` table + partial unique index                 | OK      |
| `count_active_wildcard_holders` gains `AND u.is_robot = 0`  | OK      |
| `KNOWN_CAPS` gains `robots:manage`, `robots:view`           | OK      |
| Forbidden-cap strip in `load_authed_user`                   | OK-test |
| Assignment-time forbidden-cap reject (create + role assign) | OK-test |
| SSO rejects name starting with `robot:`                     | OK-test |

Auth / cache:

| Plan item                                               | Status  |
| ------------------------------------------------------- | ------- |
| `TokenCache` rename, `TokenKind { ApiKey, RobotToken }` | OK      |
| `cbrk_` bearer dispatch in extractor                    | OK      |
| Argon2 timing parity on empty-candidate path            | OK-test |
| SHA-256 cache key, prefix lookup, cache insert          | OK      |
| Inline `mark_*_used` await + warn-swallow               | OK      |

Robot endpoints:

| Plan item                                                     | Status  |
| ------------------------------------------------------------- | ------- |
| `POST /api/admin/robots` create/revive (BEGIN IMMEDIATE)      | OK-test |
| `GET /api/admin/robots` list                                  | OK      |
| `GET /api/admin/robots/{name}`                                | OK-test |
| `DELETE /api/admin/robots/{name}` tombstone (BEGIN IMMEDIATE) | OK-test |
| 404 matrix on unknown name                                    | OK-test |
| 403 matrix on missing cap                                     | OK-test |

Audit + display identity:

| Plan item                                                  | Status    |
| ---------------------------------------------------------- | --------- |
| `display_identity()` on `routes/builds.rs` (submit/revoke) | OK        |
| `display_identity()` on `routes/admin.rs` entity handlers  | OK        |
| Lint-style test with multi-line self-tests                 | OK-test   |
| Revoke-self coexistence for `cbrk_`                        | MISS (G7) |

### R2 — rotation + coexistence (commit `cc9d331`)

| Plan item                                                        | Status    |
| ---------------------------------------------------------------- | --------- |
| `rotate_token` (BEGIN IMMEDIATE, re-read, classify)              | OK-test   |
| `POST /api/admin/robots/{name}/token` with `renew` body flag     | OK-test   |
| `DELETE /api/admin/robots/{name}/token` idempotent + 404         | OK-test   |
| `PUT /api/admin/robots/{name}/description` + 404                 | OK-test   |
| `list_entities_filtered` + `?type=user\|robot\|all`              | OK-test   |
| `?type=garbage` → 400                                            | OK-test   |
| `prefix_template_contains_username`                              | OK-test   |
| `set_entity_default_channel` rejects robot + `${username}`       | OK        |
| `submit_build` rejects robot + `${username}` resolved channel    | OK        |
| `POST /api/auth/api-keys` rejects robot caller (400)             | OK-test   |
| `POST /api/auth/tokens/revoke-all` rejects robot caller (400)    | OK-test   |
| `POST /api/auth/token/revoke` rejects robot caller (self-revoke) | MISS (G7) |

### R3 — `cbc admin robots` (commit `618cad4`)

| Plan item                                                   | Status  |
| ----------------------------------------------------------- | ------- |
| `cbc admin robots create` with `--expires` / `--no-expires` | OK      |
| Date-or-`--no-expires` clap enforcement                     | OK      |
| UTC semantics in help text                                  | OK      |
| `cbc admin robots list`                                     | OK      |
| `cbc admin robots get`                                      | OK      |
| `cbc admin robots set-description`                          | OK      |
| `cbc admin robots enable` / `disable`                       | OK      |
| `cbc admin robots roles set/add/remove`                     | OK      |
| `cbc admin robots default-channel set/clear`                | OK      |
| `cbc admin robots token new [--renew]`                      | OK      |
| `cbc admin robots token revoke --yes-i-really-mean-it`      | OK      |
| `cbc admin robots delete --yes-i-really-mean-it`            | OK      |
| `cbc admin users list` appends `?type=user`                 | OK      |
| `cbc admin users get` rejects robot email                   | OK      |
| Manual smoke workflow                                       | Not run |

### Non-code deliverables

| Item                                                | Status |
| --------------------------------------------------- | ------ |
| Design v4 file present                              | OK     |
| v1 + v2 impl review files present                   | OK     |
| Plan v2-follow-ups section + SHA refresh + F15 note | OK     |
| Plan progress table marks everything "Done\*"       | OK     |

## Commit Granularity Evaluation

Applying the `git-commits` skill (400-800 authored LOC target):

| SHA       | Commit                    | Authored LOC | Compiles alone                         | Verdict                              |
| --------- | ------------------------- | ------------ | -------------------------------------- | ------------------------------------ |
| `5d51cf9` | P1                        | ~200         | yes                                    | PASS                                 |
| `ff349b3` | P2-guard                  | ~50          | yes                                    | PASS (small but single concern)      |
| `be5afca` | P2 path reshape           | ~800         | yes                                    | PASS                                 |
| `8325bb8` | P3 token usage            | ~150         | yes                                    | PASS                                 |
| `6335e7d` | R1 robot provision + auth | ~2900        | **plain yes, `--all-targets` no** (G9) | MARGINAL                             |
| `cc9d331` | R2 rotation + coexistence | ~1100        | yes                                    | MARGINAL (above target)              |
| `618cad4` | R3 cbc admin robots       | ~1100        | yes                                    | MARGINAL (above target, bundles F15) |

All commits carry DCO sign-off and the `Co-authored-by:` trailer. Commit message
style matches Ceph convention. R1 is above the sizing target primarily because
of the `api_keys.rs` → `token_cache.rs` rename folded in — the robot feature
itself is closer to ~900 authored lines, which is within target. R2 and R3 could
have split finer (R2: rotation vs coexistence; R3: cbc surface vs the
default-channel null server-side change), but the splits would have needed a
separate `--fixup` pass on each, and the autosquash already consolidates the v1
wave.

Bisectability: see G9 — R1 passes plain `cargo check --workspace` (the
documented pre-commit gate) but fails `cargo check --workspace --all-targets`.
Every other commit in the phase builds and tests cleanly in isolation.

## Confidence Score

Starting at 100, applying the `confidence-scoring` deduction table. Each
distinct finding is scored independently:

| Deduction                                                 | Finding                                                                                                        | Points  |
| --------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------- |
| D3 Correctness bug (silent misbehaviour + misleading 200) | G7 — `revoke_token` accepts `cbrk_`, no-ops, logs synthetic email, returns 200                                 | −5      |
| D2 Missed requirement (spec)                              | G8 — `HUMAN_ONLY_ROUTES` rationale is factually wrong for `auth.rs` (related to G7)                            | −2      |
| D13 Minor bug / style                                     | G9 — R1 commit fails `cargo check --all-targets` due to test module landing before its target                  | −2      |
| D13 Minor bug / style                                     | F15 (carry-over) — R3 commit bundles `channel_id: i64 → Option<i64>` without plan amendment                    | −1      |
| D13 Minor bug / style                                     | `routes/channels.rs:650-658` retains local `is_unique_violation` copy with stale comment                       | −1      |
| D6 Test coverage (minor)                                  | G7 regression test not written (handler shape needs `HeaderMap`, slightly more than existing harness supports) | −1      |
| **Total**                                                 |                                                                                                                | **−12** |

**Score: 88 / 100**

Rationale for the delta from v2 (78 → 88): the v2 wave closed ~−20 points of
named findings — each was verified against the code, not accepted from the
plan's self-report. One new finding (G7) was surfaced during v3 review that
neither v1 nor v2 caught because the lint allowlist hid it from automated
detection. G8 is the mis-stated rationale that enabled G7 to persist; G9 is a
standing bisectability issue the v2 wave did not touch. F15 and the
`routes/channels.rs` comment are carry-over cosmetic items.

Every CRITICAL v1 finding (F1-F5) remains resolved. The concurrency tests
(`concurrent_revive_yields_exactly_one_winner`,
`concurrent_rotation_leaves_exactly_one_active_token`) regression-guard the
`BEGIN IMMEDIATE` isolation. The Argon2 timing channel (G3) is closed with a
sentinel-based regression test. The design-contract shape (F10, F11) and the
revive semantics (G4, G5) match design v4 byte-for-byte.

## Go / No-Go Recommendation

**Go** — with the following follow-ups tracked before the branch merges to
`main`. None are blockers.

### Follow-ups (ordered by priority)

1. **G7 (MODERATE)** — reject `cbrk_` bearers in `revoke_token` with a 400
   pointing at `DELETE /api/admin/robots/{name}/token`. The fix is a 4-line
   branch next to the existing `cbsk_` check; once it lands, the `auth.rs` entry
   in `HUMAN_ONLY_ROUTES` becomes correct by construction (robots are rejected
   before reaching a `user.email` log line).
2. **G8 (LOW)** — narrow or remove the `HUMAN_ONLY_ROUTES` allowlist. Minimum:
   document why each entry is safe with a cap-gate citation. Nice-to-have: move
   to function-level annotations so files with mixed human/robot-reachable
   handlers can still be lint-guarded.
3. **G9 (LOW)** — fix R1 bisectability by either (a) moving the
   `username_predicate_tests` module from R1 `channels/mod.rs` to R2
   `channels/mod.rs` via an additional autosquash, or (b) accepting the gap and
   noting it in the plan's "Autosquash SHA refresh" table so future bisect
   sessions skip past R1 when running `--all-targets`.
4. **`routes/channels.rs:650-658` (cosmetic)** — remove the local
   `is_unique_violation` copy and its stale comment (_"also defined in
   routes::admin"_ — the shared helper lives in `db::mod` now). Out of scope for
   this phase per the plan; flag for a small future commit.
5. **P3 warn-on-failure test (deferred)** — plan rationale is acceptable.
   Revisit only if another phase adds a `tracing-test` dev-dep anyway.

## Summary (Severity-Ordered)

1. **G7 — MODERATE** — `POST /api/auth/token/revoke` accepts `cbrk_` bearers,
   performs a zero-row UPDATE against the wrong table, logs the synthetic robot
   email in an actor-format `tracing::info!`, and returns 200
   `{"detail": "token revoked"}` — misleading the caller. Neither v1 nor v2
   caught this because the `HUMAN_ONLY_ROUTES` allowlist exempts `auth.rs`, and
   the rationale for that exemption ("robots can't authenticate via SSO,
   API-key, or session") overlooks the `cbrk_` bearer path.

2. **G8 — LOW** — `HUMAN_ONLY_ROUTES` rationale is too broad. `auth.rs` is not
   fully human-only (G7); `admin.rs`'s `list_entities` requires only
   `permissions:view`, which robots could in principle hold via a custom role
   (no bug today, but the regression guard is over-permissive).

3. **G9 — LOW** — R1 commit fails `cargo check --all-targets` because the test
   module calls `prefix_template_contains_username` which lands in R2. Plain
   `cargo check --workspace` (the documented pre-commit gate) passes.
   Bisectability penalty only — not a production issue.

4. **F15 — LOW (carry-over)** — R3 commit bundled the server-side
   `SetDefaultChannelBody.channel_id: i64 → Option<i64>` loosening without
   calling it out in the commit message. Compatible change; plan amendment notes
   it.

5. **`routes/channels.rs` local `is_unique_violation` copy (cosmetic)** —
   pre-dates the phase; plan explicitly defers. The comment _"also defined in
   routes::admin"_ is stale (shared helper is in `db::mod` now). One-line fix
   when opportunistically touched.

6. **G2, G3, G4, G5, G6, G1 (a + b), F18 — RESOLVED** — every v2 finding closed
   with code changes and, where applicable, regression tests. The R1 and R2
   handler-test harnesses provide a durable foundation for future coexistence
   checks.

**Confidence score: 88/100.** Recommendation: **go**. The robot-account feature
is production-ready for provisioning, authentication, rotation, and
tombstone/revive lifecycles. G7 is the only substantive follow-up; G8 is its
natural companion; G9 is a tooling/history concern. None block merging to
`main`.
