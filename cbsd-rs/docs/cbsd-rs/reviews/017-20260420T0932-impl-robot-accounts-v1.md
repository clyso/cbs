# 017 — Robot Accounts: Implementation Review v1

| Field    | Value                                                          |
| -------- | -------------------------------------------------------------- |
| Design   | `017-20260417T1130-robot-accounts.md` (v3)                     |
| Plan     | `017-20260419T2123-robot-accounts.md`                          |
| Scope    | Commits `c6ed6d3`, `7e66fb2`, `5c43229`, `447a9b8`, `dbb7cbd`, |
|          | `d00b40c` (docs + P3 + R1 + R2 + R3)                           |
| Branch   | `wip/cbsd-rs-robot-tokens`                                     |
| Reviewer | Opus 4.7                                                       |
| Date     | 2026-04-20                                                     |

## Scope Note

The plan declares all 8 commits "Done" in the progress table. The in-scope
commits on this branch cover:

- **P3** (`7e66fb2`) — token usage tracking
- **R1** (`5c43229`) — robot provision + `cbrk_` auth
- **R2** (`dbb7cbd`) — robot token lifecycle + coexistence guards
- **R3** (`d00b40c`) — `cbc admin robots` subcommand tree

Plan commits 2, 3, 4 (P1, P2-guard, P2) landed earlier on this same branch and
are out of scope for this review. No phase is formally deferred — the plan
claims full completion.

## Plan Coverage Matrix

Legend: `OK` implemented as designed, `PART` partial / wrong shape, `MISS`
missing, `DEV` deviates from design.

### P3 — token usage tracking (commit `7e66fb2`)

| Plan item                                              | Status                   |
| ------------------------------------------------------ | ------------------------ |
| migration `006_token_usage.sql` adds columns           | OK                       |
| `validate_paseto` writes `last/first_used_at`          | DEV (via `tokio::spawn`) |
| `verify_api_key` path writes `last/first_used_at`      | DEV (via `tokio::spawn`) |
| `mark_used` helper in `db/tokens.rs`, `db/api_keys.rs` | OK                       |
| swallow-and-warn on update failure                     | OK                       |
| unit test for `mark_used` idempotence                  | MISS                     |
| unit test asserting `tracing::warn!` on failure        | MISS                     |

Design and plan both say "No throttle. SQLite WAL handles the write volume";
neither says the write must be off-request-path via `tokio::spawn`. Moving it to
a spawned task is an implementer choice with two consequences worth flagging
(see F6).

### R1 — robot provision + `cbrk_` auth (commit `5c43229`)

Schema:

| Plan item                                           | Status |
| --------------------------------------------------- | ------ |
| Migration `007_robot_accounts.sql` ALTER `users`    | OK     |
| `robot_tokens` table with partial unique index      | OK     |
| `idx_robot_tokens_prefix`, `idx_robot_tokens_robot` | OK     |

Permissions / hardening:

| Plan item                                                         | Status |
| ----------------------------------------------------------------- | ------ |
| `count_active_wildcard_holders` gains `AND u.is_robot = 0`        | OK     |
| `KNOWN_CAPS` gains `robots:manage`, `robots:view`                 | OK     |
| Reject assignment of forbidden-cap roles to robots at assign-time | MISS   |
| Reject create/revive with role containing forbidden cap           | MISS   |
| SSO first-login rejects `users.name` starting with `robot:`       | MISS   |

Entity handler extensions:

| Plan item                                                  | Status    |
| ---------------------------------------------------------- | --------- |
| `deactivate_entity` branches on `is_robot` (disable-only)  | OK        |
| Cache purged in both branches                              | OK        |
| `activate_entity` branches on `is_robot` + token existence | PART      |
| — tokenless robot must return **400** per plan             | DEV (409) |
| — purge cache on activate (plan mandates)                  | MISS      |
| `GET /api/admin/entities?type=user\|robot\|all`            | OK (R2)   |

Auth / cache generalisation:

| Plan item                                                    | Status |
| ------------------------------------------------------------ | ------ |
| `ApiKeyCache` → `TokenCache`; `CachedApiKey` → `CachedToken` | OK     |
| `TokenKind { ApiKey, RobotToken }` discriminator             | OK     |
| `AppState.api_key_cache` → `AppState.token_cache`            | OK     |
| `cbrk_` bearer dispatch                                      | OK     |
| SHA-256 cache key, prefix lookup, Argon2id verify            | OK     |
| `last_used_at` write on verified robot token                 | OK     |
| `expires_at` NULL → never expires                            | OK     |
| Forbidden-cap strip in `load_authed_user`                    | OK     |
| `AuthUser { is_robot }` field                                | OK     |
| `AuthUser::display_identity()`                               | OK     |
| `whoami` gains `is_robot`                                    | OK     |
| Build response `is_robot` on submit/get/list                 | MISS   |

Robot endpoints and CLI (server-side):

| Plan item                                                     | Status |
| ------------------------------------------------------------- | ------ |
| `POST /api/admin/robots` create / revive                      | PART   |
| — `BEGIN IMMEDIATE` isolation                                 | MISS   |
| — re-read row under lock for concurrent-revive detection      | MISS   |
| — revive resets `default_channel_id = NULL`                   | MISS   |
| — revive resets `created_at = unixepoch()`                    | MISS   |
| — revive resets `name = robot:<name>`                         | MISS   |
| — request body `expires: "YYYY-MM-DD"\|"infinity"`            | DEV    |
| `GET /api/admin/robots` list                                  | PART   |
| — response includes `token_state`, `token_expires_at`,        | MISS   |
| `last_used_at`, `display_name`                                |        |
| `GET /api/admin/robots/{name}` details                        | PART   |
| — response missing `token_status`, `roles`, `effective_caps`, | MISS   |
| `display_name`                                                |        |
| `DELETE /api/admin/robots/{name}` tombstone                   | PART   |
| — `BEGIN IMMEDIATE` isolation                                 | MISS   |
| — response populated with revoked count                       | OK     |
| — design says 404 on unknown name (plan R2); OK               | OK     |

Audit / display identity:

| Plan item                                                         | Status |
| ----------------------------------------------------------------- | ------ |
| `tracing::info!` switched to `display_identity()` in admin routes | PART   |
| Lint-style test preventing `user.email` in log macros             | MISS   |

### R2 — token rotation + coexistence (commit `dbb7cbd`)

| Plan item                                                          | Status |
| ------------------------------------------------------------------ | ------ |
| `rotate_token(tx, ...)` DB helper                                  | OK     |
| `POST /api/admin/robots/{name}/token` with `renew` body flag       | PART   |
| — `BEGIN IMMEDIATE` on the rotation transaction                    | MISS   |
| — request body field is `expires_at` (epoch) not `expires` date    | DEV    |
| `DELETE /api/admin/robots/{name}/token` — idempotent 200           | OK     |
| — 404 on unknown name                                              | OK     |
| `PUT /api/admin/robots/{name}/description`                         | OK     |
| — 404 on unknown name                                              | OK     |
| `list_entities_filtered` + `?type=user\|robot\|all`                | OK     |
| — 400 on invalid filter value                                      | OK     |
| `prefix_template_contains_username` predicate                      | OK     |
| `set_entity_default_channel` rejects robot + `${username}` channel | OK     |
| `submit_build` rejects robot + `${username}` resolved channel      | OK     |
| `POST /api/auth/api-keys` rejects when caller `is_robot`           | OK     |
| — design also required rejection when **target** `is_robot = 1`    | PART   |
| (moot in practice: handler only supports own self-creation)        |        |
| `POST /api/auth/tokens/revoke-all` rejects robot caller            | OK     |
| Tests (token rotation, coexistence, 404 matrix, filter)            | MISS   |

### R3 — `cbc admin robots` (commit `d00b40c`)

| Plan item                                                             | Status |
| --------------------------------------------------------------------- | ------ |
| `cbc admin robots create` with `--expires`, `--description`, `--role` | PART   |
| — `--expires` is optional in the CLI; design says required            | DEV    |
| — no `"infinity"` literal support                                     | MISS   |
| — `parse_expires` computes **midnight UTC on the given day**,         | DEV    |
| not the **day after** as the design requires                          | (BUG)  |
| — help text does NOT state UTC semantics                              | MISS   |
| `cbc admin robots list`                                               | OK     |
| `cbc admin robots get`                                                | PART   |
| — no token status, no roles, no effective caps displayed              | MISS   |
| `cbc admin robots set-description`                                    | OK     |
| `cbc admin robots enable` / `disable`                                 | OK     |
| `cbc admin robots roles set/add/remove`                               | OK     |
| `cbc admin robots default-channel set/clear`                          | OK     |
| `cbc admin robots token new [--renew]`                                | PART   |
| — `--expires` optional; design silent but inconsistent with create    |        |
| `cbc admin robots token revoke --yes-i-really-mean-it`                | OK     |
| `cbc admin robots delete --yes-i-really-mean-it`                      | OK     |
| `cbc admin users list` appends `?type=user`                           | OK     |
| `cbc admin users get` rejects robot email                             | OK     |
| Manual smoke workflow in plan                                         | ?      |

### Tests (design + plan mandate)

The plan section "Tests (critical paths)" for R1 and the test lists for R2
enumerate **22+ distinct tests** (robot name validation, create/revive happy
paths, 409 on active, 409 on human collision, concurrent revive race,
forbidden-cap assignment reject, SSO forgery guard, wildcard-count filter, all
auth extractor paths, role-mutation cache-hit/miss strip, disable/enable cycle,
tombstone, display-identity unit, lint-style test, token rotation with `renew`
matrix including expired-not-revoked, concurrent rotation, non-existent name 404
matrix, description clear/set, all coexistence rejections, `?type` filter cases
including garbage).

Observed in tree: **zero tests touching any robot code path.** No `robots` test
module, no `#[cfg(test)]` in `db/robots.rs`, no test covering
`validate_robot_name` even though it is pure and trivial to test. This is a
**critical, across-the-board** gap.

## Findings (Severity-Ordered)

### F1 — CRITICAL: `BEGIN IMMEDIATE` isolation missing everywhere

The design (§ Authentication Flow, § REST API) and plan (commits 6, 7) name
`BEGIN IMMEDIATE` **six times** for the create, revive, tombstone, and rotate
transactions. Rationale: avoid the read-then-write race on `users.active` and
eliminate the raw `SQLITE_CONSTRAINT_UNIQUE` surface on the partial unique
index.

Observed: every transaction uses plain `state.pool.begin()` (sqlx's default is
`BEGIN DEFERRED`). Files:

- `routes/robots.rs` `create_or_revive_robot` line 161
- `routes/robots.rs` `create_or_rotate_token` line 435
- `db/robots.rs` `tombstone_robot` line 291 (also not `IMMEDIATE`)

Consequences at runtime:

- Two concurrent `POST /api/admin/robots` for the same tombstoned name can both
  pass the "existing.active == false" check and race into `revive_robot_in_tx`;
  the loser will hit the partial unique index and bubble a 500
  (`is_unique_violation` is only consulted in the `create_robot_in_tx` arm, not
  the revive arm). Plan R1 test item "Concurrent revive … returns 409, not a raw
  constraint violation" is **not satisfied**.
- Two concurrent `POST /api/admin/robots/{name}/token` with `renew=true`
  similarly race and the loser surfaces a 500 — plan R2 test item "Concurrent
  rotation … Neither request returns 500" is not satisfied.

Fix: `let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await` — or execute
`BEGIN IMMEDIATE` as raw SQL via `sqlx::Executor::execute("BEGIN IMMEDIATE")`
and rely on a drop guard to rollback. Must be applied to all four call sites
above.

### F2 — CRITICAL: revive does not reset account state per spec

Design § REST API step 4: "Update `users`: `active = 1`, `name = robot:<name>`,
`robot_description` = request value or `NULL`, `default_channel_id = NULL`,
`created_at = unixepoch()`."

`db::robots::revive_robot_in_tx` (`db/robots.rs` lines 244–251) only updates
`active`, `robot_description`, `updated_at`. Missing:

- `name` not re-written — if for any reason a prior mutation changed it (there
  is no such path today, but the design is explicit)
- `default_channel_id` is **not cleared** — a revived robot inherits the prior
  identity's default channel, which is exactly the drift the design calls out in
  § "Channel Template Constraints" and § "revive"
- `created_at` not reset — `GET /api/admin/robots/{name}` will return a stale
  creation timestamp on revived robots, confusing audit trails

This is a silent data-correctness bug. A robot revived over a tombstone whose
prior identity had `default_channel_id` set to a `${username}` channel will now
be authenticated **and** pointed at a forbidden channel; subsequent builds will
be rejected at submission time (F12 predicate catches it), but the operator
experience is "I just created this robot fresh and it cannot build."

### F3 — CRITICAL: revive does not re-read under lock (design step 1)

Design § REST API step 1: "Re-read the current `users` row under the write lock.
If the row is now `active = 1` (a concurrent revive committed first), abort with
409 Conflict."

Observed: `create_or_revive_robot` reads the row **before** opening the
transaction (line 150), then opens a DEFERRED transaction and dispatches. There
is no re-read under write lock. Combined with F1, this is the exact TOCTOU race
the design called out.

Even after switching to `BEGIN IMMEDIATE`, the re-read step is still required —
`BEGIN IMMEDIATE` serialises writers but the pre-transaction read can still
observe stale state.

### F4 — CRITICAL: no tests for any robot code path

Plan R1 and R2 enumerate ~22 critical-path tests. None exist. This alone would
block landing in a stricter shop. At minimum, the concurrent-revive and
concurrent-rotation race tests are non-optional — they are the only way to
regression-catch F1/F3.

Mitigation: the tests that don't require the server harness (i.e.,
`validate_robot_name`, `name_to_synthetic_email` round-trip,
`prefix_template_contains_username`, `AuthUser::display_identity`) can be added
as unit tests inside the respective modules with no infrastructure cost.

### F5 — CRITICAL: expiry-date off-by-one-day bug in cbc

Design § Token Design "Expiry":

> ISO calendar date `YYYY-MM-DD`. The token is valid through the end of that UTC
> day; stored as Unix epoch seconds of `00:00:00 UTC` on the day **after** the
> given date.

`cbc/src/admin/robots.rs` `parse_expires` (line 285):

```rust
let ts = date.and_hms_opt(0, 0, 0).ok_or(...)?.and_utc().timestamp();
```

This produces epoch for `YYYY-MM-DD 00:00:00 UTC` — the start of the given day,
not the day after. A token created with `--expires 2026-12-31` will be rejected
by the auth path at `2026-12-31 00:00:01 UTC`, **one full day earlier** than the
design contract. Every `cbc`-issued robot token with an explicit expiry is wrong
by one day.

Fix: `date.succ_opt()...` or add 86400 to the timestamp.

### F6 — IMPORTANT: `tokio::spawn` for usage tracking changes semantics

The plan (commit 5 "Key details") says:

> `mark_used` is invoked outside any request-path transaction. A failed
> usage-update must not fail the request — the error is swallowed, but every
> swallow emits `tracing::warn!` with the failing cause and the token id. No
> bare `let _ = ...` without a log line.

The implementer chose `tokio::spawn(async move { ... })` instead of awaiting
inline. Implications:

1. **Shutdown race.** During a graceful shutdown, spawned tasks may not
   complete. A batch of final auth events will not have their `last_used_at`
   updated. Not critical, but worth noting.
2. **Runtime overhead.** Spawn + pool-checkout per successful auth is more
   overhead than an inline `await`. At the stated "auth rate well below the
   threshold where batching would be beneficial", an inline await is strictly
   cheaper.
3. **Lost errors in tests.** A test asserting the tracing warn-on-failure (plan
   requires this) has to `tokio::yield_now().await` multiple times to see it —
   flaky.

Neither design nor plan authorises the spawn; inline await matches both
documents' description ("on every successful authentication", "never fail the
request"). Recommend converting to inline `await` with the warn-swallow pattern.

### F7 — IMPORTANT: missing forbidden-cap assign-time reject

Design § Permissions Model / plan R1 "Permissions hardening":

> Reject assignment of roles containing forbidden caps to robot targets on
> `POST /api/admin/entity/{email}/roles`.

And in `create_or_revive_robot`:

> validates that each requested role does not contain a forbidden cap for robot
> targets.

Observed: only existence of each role is checked (`routes/robots.rs` line 121).
Neither `POST /api/admin/entity/{email}/roles` nor the robot-creation path
inspects the role's `role_caps` against `ROBOT_FORBIDDEN_CAPS`.

The design explicitly calls this "defense in depth": the auth-time strip is
layer 1; this assignment-time check is layer 2. Today only layer 1 exists. The
practical consequence is that a human admin can assign (for example) a
wildcard-holding role to a robot and see no error from the server; the forbidden
cap is then silently stripped every auth. This is the exact observability hazard
the design flagged as undesirable.

### F8 — IMPORTANT: SSO forgery guard absent

Design § Identity Model:

> The human-account creation path (SSO first-login) rejects any `users.name`
> value that starts with `robot:`, preventing identity forgery via OAuth
> display-name shenanigans.

No such check exists in `routes/auth.rs` OAuth callback or
`db/users.rs::create_or_update_user`. Low practical risk (Google display names
starting with `robot:` are possible but unusual), but the design is specific and
the implementation is trivial (one `if` inside `create_or_update_user`).

### F9 — IMPORTANT: `activate_entity` returns 409, not 400

Plan commit 6 Entity `activate` / enable cycle tests:

> Entity `activate` on a tombstoned robot (active=0, no non-revoked tokens):
> **400** with a message pointing at `POST /api/admin/robots`.

`routes/admin.rs` `activate_entity` returns `StatusCode::CONFLICT` (409). Minor,
but a spec deviation that will break the test if/when the test exists.

### F10 — IMPORTANT: `GET /api/admin/robots/{name}` response is incomplete

Design § REST API "Get Robot" shows:

```json
{
  "name": ..., "display_name": ..., "description": ..., "active": ...,
  "created_at": ...,
  "token_status": { "state", "prefix", "expires_at", "first_used_at",
                    "last_used_at", "token_created_at" },
  "roles": [...],
  "effective_caps": [...]
}
```

Observed `RobotItem` (`routes/robots.rs` line 87) has seven fields total:
`name`, `email`, `description`, `active`, `has_token`, `created_at` (plus
`email` which is not in the spec). Missing: `display_name`, `token_status`
object, `roles`, `effective_caps`.

This makes the feature **unusable for its stated audit purpose** — an admin
cannot see the expiry of a robot's token, cannot see when it was last used,
cannot see the roles it holds, and cannot see the effective cap set. All the P3
usage-tracking work is collected into the DB but is unreachable from any read
API.

The `cbc admin robots get` output (line 447) is correspondingly thin.

### F11 — IMPORTANT: create / rotate request body uses `expires_at` (epoch int)

Design § REST API: `"expires": "2027-04-17"` (string) or `"infinity"`. Plan
commit 6 "Create or Revive Robot — `POST /api/admin/robots`" body field is
`expires`.

Observed: request body struct `CreateRobotBody { expires_at: Option<i64> }`
(`routes/robots.rs` line 41) and `RotateTokenBody { expires_at: Option<i64> }`
(line 64). The wire contract expects an epoch integer; `"infinity"` is expressed
as the absence of the field.

Two problems:

1. Spec deviation — any external client following the design document will post
   `expires` (string) and receive a 400 or silently omit the field.
2. The UTC-date semantics reasoning (including the day-after offset enforced in
   F5) has to be duplicated in every client. The server-side intent was to own
   the "string date → epoch" translation so every client sees consistent
   behaviour; this implementation pushes it onto each client, which is precisely
   why `cbc` has the F5 bug.

Fix: accept either a `YYYY-MM-DD` string or the literal string `"infinity"` on
the server side, compute the next-day midnight UTC epoch, and store. Clients
send strings.

### F12 — IMPORTANT: duplication between API key and robot token code paths

`auth/token_cache.rs` is a known duplication hotspot flagged in the review
brief. Observed:

- `verify_api_key` (lines 206–269) and `verify_robot_token` (lines 314–377) are
  structurally identical: cache lookup, prefix lookup, `spawn_blocking` Argon2
  loop, expiry check, cache insert. The only differences are the literal
  `"cbsk_"` vs `"cbrk_"`, the DB call (`db::api_keys::find_api_keys_by_prefix`
  vs `db::robots::find_active_token_by_prefix`), and the `TokenKind` enum
  variant. ~120 lines of boilerplate.
- `generate_api_key_material` (lines 276–288) and
  `generate_robot_token_material` (lines 298–310) are identical except for the
  prefix literal — 13 lines each, 100% token-for-token duplicated.
- In `auth/extractors.rs`, the three branches for API key / robot token / PASETO
  all repeat the fire-and-forget spawn pattern (lines 278–287, 302–311,
  237–242). One generic `spawn_mark_used` helper would collapse all three.
- Tombstone-revoke-all-tokens logic appears three times:
  `db::robots::tombstone_robot` (line 300), `db::robots::rotate_token` (line
  320), `routes/robots.rs::revoke_robot_token` inline sqlx (line 511). The third
  is especially jarring — a raw sqlx query in the route handler that should
  delegate to `db::robots`.

Recommended refactor — a single
`verify_hashed_token<T>(cache, raw, prefix, find_fn) -> CachedToken` function
parameterised by prefix/kind/ finder. The two verify fns become ~5-line
wrappers.

### F13 — MODERATE: `INSERT OR IGNORE` inconsistency between create and revive

- `create_robot_in_tx` inserts roles with `INSERT OR IGNORE` (line 220).
- `revive_robot_in_tx` inserts roles with `INSERT` (line 276).

Both paths have just cleared any prior rows, so `OR IGNORE` is defensively-safe
and `INSERT` would error on a duplicate in the request list. Pick one. Recommend
`INSERT OR IGNORE` in both (tolerant of duplicated request input) or deduplicate
the input in the handler.

### F14 — MODERATE: `tokens_revoked` UPDATE affects rows under FK cascade risk

`tombstone_robot` runs two UPDATEs then commits. If an admin later runs the
`sqlx` cleanup that deletes the `users` row (theoretically possible via a future
admin path — design forbids it but the FK is `ON DELETE CASCADE`), all
`robot_tokens` rows disappear. Design states hard-delete never happens, so this
is latent, not active. Worth a comment in the schema migration stating the
CASCADE is defensive only.

### F15 — LOW: commit 8 bundles a server change not in its plan scope

Plan for commit 8 is "Mostly additive surface area with some subtle URL plumbing
in `cbc admin users`". The server-side `SetDefaultChannelBody` change from
`channel_id: i64` to `channel_id: Option<i64>` (+20 line handler rewrite) lands
in this commit. Justification in the commit message exists ("The server's
default-channel endpoint is extended to accept null so the CLI can clear a
robot's channel assignment"), but the plan wasn't amended; this should have been
reflected in the plan or split into a separate commit. Minor. Also note: this is
a **breaking API change** (previously the field was required; now `null` is
accepted) that the commit message does not explicitly flag as breaking.

### F16 — LOW: `P3` commit `tokio::spawn` uses `state.pool.clone()`

Per-auth pool clones are cheap (Arc bump) but every `spawn` still adds a
pool-checkout race under load. If the pool is near saturation (design invariant:
`max_connections = 4`), a burst of auths can queue usage-update writes behind
real request traffic. Inline await (see F6) avoids this because the connection
is already held.

### F17 — LOW: plan size estimate vs actual

- Commit 5 (P3) — plan 150 lines, actual +86/-2. Within the "below 200, consider
  whether meaningful alone" cbsd-rs guideline; the commit is meaningful and the
  small size is fine.
- Commit 6 (R1) — plan 900-1100, actual ~1089 (excl. `.sqlx`). On target.
- Commit 7 (R2) — plan 400-500, actual ~539. On target.
- Commit 8 (R3) — plan 400-500, actual ~843. Above target; would be within
  target if the server-side default-channel change (F15) were split out.

### F18 — LOW: `is_unique_violation` helper uses string comparison

`routes/robots.rs` line 588:

```rust
db_err.code().is_some_and(|c| c == "2067")
```

Code `2067` is `SQLITE_CONSTRAINT_UNIQUE`. The comparison is correct but SQLite
extended error codes are more commonly compared as integers or via a sqlx enum.
Not functionally wrong, just brittle (a future sqlx update that returns the code
differently would break the constraint-mapping silently). Consider
`e.as_database_error().and_then(|e| e.code())` + a named constant.

## Commit Granularity Evaluation

Applying the `git-commits` skill (400-800 authored LOC target):

| #   | Commit       | Authored LOC | Compiles alone | Meaningful alone                                       | Verdict                                                                      |
| --- | ------------ | ------------ | -------------- | ------------------------------------------------------ | ---------------------------------------------------------------------------- |
| 5   | `7e66fb2` P3 | 86           | yes            | yes — symmetric tracking lands everywhere              | **PASS** (below target but tightly coupled single concern)                   |
| 6   | `5c43229` R1 | ~1089        | yes            | yes — robot provision + auth end-to-end                | **PASS** (slightly above mid-target; tightly coupled per plan sizing note)   |
| 7   | `dbb7cbd` R2 | ~539         | yes            | yes — token rotation + coexistence                     | **PASS**                                                                     |
| 8   | `d00b40c` R3 | ~843         | yes            | yes — but bundles an unrelated server API change (F15) | **MARGINAL** — would be PASS if the default-channel server change were split |

Commit messages: all follow the `component: description` prefix convention.
Bodies explain "why" — appropriate for Ceph-style commits.

No commit violates the "compiles alone" rule — each of these commits leaves the
workspace building cleanly (verified with
`SQLX_OFFLINE=true cargo check --workspace` on current HEAD).

## Confidence Score

Applying the `confidence-scoring` skill with the full deduction table. Start
at 100.

| Deduction                      | Finding(s)                                                                                                               | Points  |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------ | ------- |
| D2 Missed requirement (spec)   | F2 revive field reset, F7 assign-time reject, F8 SSO forgery guard, F10 GET response incomplete, F11 expires wire format | −25     |
| D3 Correctness bug             | F1 no `BEGIN IMMEDIATE` (race), F3 no re-read under lock, F5 cbc off-by-one-day, F9 wrong status code                    | −20     |
| D6 Test coverage               | F4 zero robot tests                                                                                                      | −12     |
| D8 Duplication                 | F12 verify*\* and generate*\* duplicated ~130 lines                                                                      | −5      |
| D11 Dead code / unclear intent | F6 spawn vs inline                                                                                                       | −3      |
| D13 Minor bug / style          | F13 INSERT OR IGNORE inconsistency, F15 commit bundling, F16 pool-clone, F18 string code compare                         | −4      |
| **Total**                      |                                                                                                                          | **−69** |

**Score: 31 / 100**

Context on the number: the implementation compiles, the happy path works, and
the refactor to a unified token cache is clean. But the design / plan explicitly
called out concurrency (BEGIN IMMEDIATE, re-read under lock), contract-shape
(token_status, roles, effective_caps, date strings), and test coverage as core
acceptance criteria, and all three are absent. The off-by-one-day bug (F5) is a
user-visible production bug even on the happy path.

## Go / No-Go Recommendation

**No-go for "Done" status; go for `R1-partial / R2-partial / R3-partial` with a
required follow-on commit.**

The plan progress table should not show commits 6, 7, 8 as "Done" until at
minimum the following land:

### Required before declaring the feature done

1. **F1 + F3** — apply `BEGIN IMMEDIATE` and the in-transaction re-read to
   `create_or_revive_robot`, `create_or_rotate_token`, and `tombstone_robot`.
   Add a sqlx mapper for `SQLITE_CONSTRAINT_UNIQUE` on the revive arm.
2. **F2** — fix `revive_robot_in_tx` to reset `default_channel_id`,
   `created_at`, and explicitly re-write `name`.
3. **F5** — fix `cbc admin robots::parse_expires` to produce the day-after
   epoch. Add a unit test.
4. **F10 + F11** — switch the create/rotate wire format to accept `"YYYY-MM-DD"`
   / `"infinity"` strings and extend `GET /api/admin/robots/{name}` to return
   `token_status`, `roles`, `effective_caps`, `display_name`. The
   `cbc admin robots get` output must display these.
5. **F4 (subset)** — at minimum the two concurrent-race tests (revive,
   rotation), plus `validate_robot_name` table-driven test, plus
   `AuthUser::display_identity` test. These are the regression guards for
   F1/F3/F5.

### Should-fix-next-commit

6. **F7** — assign-time forbidden-cap reject on both the robot-create path and
   `POST /api/admin/entity/{email}/roles`.
7. **F8** — SSO forgery guard in `create_or_update_user`.
8. **F9** — return 400 (not 409) for tokenless-robot activate.
9. **F6** — collapse the three `tokio::spawn` blocks to inline await.
10. **F12** — deduplicate `verify_api_key` / `verify_robot_token` and
    `generate_*_material` behind a shared generic.
11. **F13** — normalise `INSERT OR IGNORE` vs `INSERT` between create and
    revive.

### Nice-to-have

12. Build response `is_robot` field (design-required).
13. `cbc --expires infinity` literal support.
14. Plan update documenting the commit-8 default-channel server change and
    flagging it as a breaking API change.
