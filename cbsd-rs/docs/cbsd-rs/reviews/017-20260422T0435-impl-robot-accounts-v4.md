# 017 — Robot Accounts: Implementation Review v4

| Field    | Value                                                                                           |
| -------- | ----------------------------------------------------------------------------------------------- |
| Design   | `017-20260417T1130-robot-accounts.md` (v4)                                                      |
| Plan     | `017-20260419T2123-robot-accounts.md`                                                           |
| Scope    | Commits `79f26d1..635a2c8` (14 commits — docs, P1, P2-guard, P2, P3, R1, R2, R3, v1/v2/v3 docs) |
| Branch   | `wip/cbsd-rs-robot-tokens` at `635a2c8`                                                         |
| Base     | `main`                                                                                          |
| Reviewer | Opus 4.7 (1M context)                                                                           |
| Date     | 2026-04-22                                                                                      |

## Scope Note

The scope runs from `79f26d1` (phase bootstrap docs) through `635a2c8` (v3
review doc). Enumerated (oldest → newest):

| SHA       | Subject                                                                       |
| --------- | ----------------------------------------------------------------------------- |
| `79f26d1` | cbsd-rs/docs: add robot accounts design, reviews, and plan                    |
| `5d51cf9` | cbc: require --yes-i-really-mean-it on irreversible commands (P1)             |
| `ff349b3` | cbsd-rs/server: deduplicate last-admin guard into shared db helper (P2-guard) |
| `be5afca` | cbsd-rs: reshape account-level admin endpoints under /api/admin/entity (P2)   |
| `8325bb8` | cbsd-rs: track first_used_at / last_used_at on tokens and API keys (P3)       |
| `2a2d672` | cbsd-rs: robot account provision and cbrk\_ bearer auth (R1)                  |
| `1561f48` | cbsd-rs/docs: mark R1 done in plan progress table                             |
| `d4d0478` | cbsd-rs/server: add robot token lifecycle and coexistence guards (R2)         |
| `534231b` | cbc: add admin robots subcommand tree (R3)                                    |
| `56492c0` | cbsd-rs/docs: add implementation review for robot accounts (v1)               |
| `70def66` | cbsd-rs/docs: plan progress + review v1 follow-ups section                    |
| `953c653` | cbsd-rs/server: cargo fmt post-autosquash                                     |
| `1d5e087` | cbsd-rs/docs: add v2 implementation review for robot accounts                 |
| `635a2c8` | cbsd-rs/docs: add v3 implementation review for robot accounts                 |

The v3 review scored this branch at 88/100 with three open findings (G7, G8, G9)
plus two carry-over cosmetic items (F15, `routes/channels.rs` duplicate).
Between v3 and v4 the user applied a fixup wave of four code/doc fixes plus one
interactive rebase on R1 that moved the sqlx offline cache entry for the
`revive_deletes_prior_robot_tokens_rows` test into R1. This review evaluates the
post-fixup state at branch tip. Every claim in the brief was cross-checked
against the actual code, not the plan's self-report and not the v1/v2/v3
reviews.

### Build & test at branch tip

```text
$ SQLX_OFFLINE=true cargo check --workspace --all-targets  # OK, no warnings
$ SQLX_OFFLINE=true cargo clippy --workspace --all-targets # OK, no warnings
$ cargo fmt --all --check                                  # OK
$ SQLX_OFFLINE=true cargo test --workspace
  test result: ok. 104 passed; 0 failed (cbsd-server)
  test result: ok.  21 passed; 0 failed (cbc)
  test result: ok.  12 passed; 0 failed (cbsd-worker)
  test result: ok.   6 passed; 0 failed (cbsd-proto)
```

## Verification of v3 Follow-ups

Every claim in the brief's "specific claims to verify" list was checked against
the actual code.

| v3 Finding                | Claim                                                               | Status at tip | Evidence                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ------------------------- | ------------------------------------------------------------------- | ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| G7                        | `revoke_token` rejects `cbrk_` bearers with 400 + two handler tests | CLOSED        | `routes/auth.rs:408-413` adds the `cbrk_` branch mirroring the existing `cbsk_` branch (`:402-407`). Test `revoke_token_rejects_cbrk_bearer_with_400` (`:685-706`) invokes the handler with a real `HeaderMap` carrying `Authorization: Bearer cbrk_...`, exercising the actual code path. Companion test `revoke_token_rejects_cbsk_bearer_with_400` (`:709-730`) pins the `cbsk_` branch as a regression guard.          |
| G8                        | `HUMAN_ONLY_ROUTES` rationale updated for `auth.rs`                 | CLOSED        | `routes/audit_identity_lint.rs:33-39` now states: "Every handler in `auth.rs` that logs `user.email` rejects non-human callers upfront: `/token/revoke` returns 400 for both `cbsk_` and `cbrk_` bearers; `/tokens/revoke-all` and `/api-keys` reject `is_robot` callers before any logging. This is enforced in code, not merely by cap design." The narrow rationale is factually accurate for the three named handlers. |
| G9 part (a)               | `username_predicate_tests` moved from R1 to R2                      | CLOSED        | `git log 79f26d1^..HEAD -- cbsd-server/src/channels/mod.rs` shows only `d4d0478` (R2) touches the file; R1 (`2a2d672`) does not diff it. The `username_predicate_tests` module lives at `channels/mod.rs:43-66` alongside the `prefix_template_contains_username` function it calls.                                                                                                                                       |
| G9 part (b)               | sqlx offline cache entry `query-21e262430b73...json` moved to R1    | CLOSED        | `git diff-tree 2a2d672` (R1) lists `cbsd-rs/.sqlx/query-21e262430b73d5f02b8ff2639551c7cb9078e5909d2a5a427be4bfc740a4cccf.json` as added. This is the cached form of the `SELECT token_prefix, revoked FROM robot_tokens ...` query used by the R1 test `revive_deletes_prior_robot_tokens_rows`.                                                                                                                           |
| Stale channels.rs comment | Comment cites `db::is_unique_violation`, not `routes::admin`        | CLOSED        | `routes/channels.rs:650-652`: "NOTE: duplicates `db::is_unique_violation` but pre-dates the shared helper and lives outside the robot-accounts phase's scope. Safe to migrate to `db::is_unique_violation` in a future opportunistic cleanup." Accurate and non-misleading.                                                                                                                                                |
| v3 review                 | Standalone docs-only commit at branch tip                           | CLOSED        | `635a2c8` is docs-only (`1 file changed, 495 insertions(+)`), adding only `017-20260421T2307-impl-robot-accounts-v3.md`. No code changed.                                                                                                                                                                                                                                                                                  |

### G7 regression coverage — quality check

The two added handler tests are well-constructed:

- Each uses the real `HeaderMap` type with a `HeaderValue::from_static` for
  `"Authorization: Bearer cbrk_<hex>"` (or `cbsk_`), so the prefix-match branch
  at `routes/auth.rs:402` / `:408` actually fires.
- Each also builds a concrete `AuthUser` via the `auth_user()` helper in
  `test_support.rs` so the extractor prelude (the
  `headers.get("authorization").is_none()` guard at `:387`) is satisfied.
- The `revoke_token_rejects_cbrk_bearer_with_400` test passes `is_robot=true`
  and the companion passes `is_robot=false` — both are meaningful shapes.
- The harness was not broken: `cargo test --workspace` passes at all three
  feature-commit tips (R1 82, R2 104, R3 104 cbsd-server tests).

No synthesized shortcuts bypass the `cbrk_` / `cbsk_` branch logic. The tests
exercise the actual code paths.

## Per-Commit Bisectability

Running `SQLX_OFFLINE=true cargo check --workspace --all-targets` on each commit
in scope:

| SHA       | Subject                   | check --all-targets | Notes                                                       |
| --------- | ------------------------- | ------------------- | ----------------------------------------------------------- |
| `5d51cf9` | P1                        | OK                  | clean                                                       |
| `ff349b3` | P2-guard                  | OK                  | clean                                                       |
| `be5afca` | P2                        | OK                  | clean                                                       |
| `8325bb8` | P3                        | OK                  | clean                                                       |
| `2a2d672` | R1                        | OK (1 warning)      | `revoke_all_active_tokens` pool-wrapper has no caller in R1 |
| `1561f48` | docs (plan progress)      | OK (1 warning)      | inherits R1 warning (docs-only commit)                      |
| `d4d0478` | R2                        | OK                  | clean — R2 introduces the first caller                      |
| `534231b` | R3                        | OK                  | clean                                                       |
| `953c653` | cargo fmt post-autosquash | OK                  | clean                                                       |
| `635a2c8` | v3 review                 | OK                  | clean                                                       |

The v3 review's G9 (R1 failing `--all-targets`) is **fully resolved**. R1 now
compiles cleanly as a hard error gate; the remaining dead-code warning at R1 is
a new, separate finding (see G10 below).

### Per-commit test runs

`cargo test --workspace` at each of the three feature commits:

| SHA       | Commit | cbsd-server tests | cbc tests | cbsd-worker tests |
| --------- | ------ | ----------------- | --------- | ----------------- |
| `2a2d672` | R1     | 82 passed, 0 fail | 21 / 0    | 12 / 0            |
| `d4d0478` | R2     | 104 / 0           | 21 / 0    | 12 / 0            |
| `534231b` | R3     | 104 / 0           | 21 / 0    | 12 / 0            |

Bisectability is a GO at every commit in scope.

## New Findings

### G10 — LOW: R1 introduces pre-extracted pool wrapper with no caller at R1

`cbsd-server/src/db/robots.rs:199-202` at R1 (`2a2d672`):

```rust
pub async fn revoke_all_active_tokens(
    pool: &SqlitePool, email: &str
) -> Result<u64, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    revoke_all_active_tokens_in_conn(&mut conn, email).await
}
```

This is the pool-acquiring wrapper around `revoke_all_active_tokens_in_conn`. At
R1, the `_in_conn` variant has callers inside the module
(`tombstone_robot_inner`), but the pool-wrapper has no caller — R2's
`routes/robots.rs:640` (the `DELETE /api/admin/robots/{name}/token` handler) is
the first. `cargo check --all-targets` at R1 emits one `#[warn(dead_code)]` on
this symbol.

Against the `git-commits` skill smell test #5 (_"No dead code: every function
added in this commit has at least one caller or reader in the same commit"_)
this is a D12 violation. The function was pre-extracted to avoid an R2 diff hunk
inside `db/robots.rs`, but at the cost of R1 carrying dead code.

**Severity calibration:** No `deny(warnings)` or `-D warnings` exists in
`Cargo.toml`, `.lefthook.yaml`, or `.github/workflows/release-cbsd-rs.yaml`. The
warning is informational — R1's `cargo check` gate still passes. The
bisectability penalty is zero in practice; only strict CI (which this project
does not run) would flag it.

**Fix options:** (a) move the pool-wrapper to R2 alongside its first caller; (b)
accept the warning; (c) add `#[cfg(test)]` or `#[allow(dead_code)]` on the
wrapper at R1 and remove the attribute at R2 — worst option, hides real dead
code. Option (a) is cleanest but requires another rebase; option (b) is what's
currently shipped and is acceptable for a LOW finding.

### G11 — LOW: Plan's "Autosquash SHA refresh" and v2-follow-ups tables are stale

`docs/cbsd-rs/plans/017-20260419T2123-robot-accounts.md` lines 863-869 still
list the **post-v1** SHAs as current:

```
| Original SHA | Post-squash target | Role                                             |
| ------------ | ------------------ | ------------------------------------------------ |
| `7e66fb2`    | `766a1b7`          | P3 token usage tracking                          |
| `5c43229`    | `f08b841`          | R1 robot provision + `cbrk_` auth                |
| `dbb7cbd`    | `b1bd180`          | R2 rotation + coexistence guards                 |
| `d00b40c`    | `331cbae`          | R3 `cbc admin robots`                            |
```

Current post-v3 targets at branch tip are `8325bb8` / `2a2d672` / `d4d0478` /
`534231b`. The "Review v2 Follow-ups" table at lines 890-900 also references the
post-v1 SHAs (`766a1b7`, `f08b841`, `b1bd180`, `b14b413`).

Readers using the plan as a navigational index will hit dead SHAs. The v3
review's own SHA list (`017-20260421T2307-impl-robot-accounts-v3.md` § "Scope
Note") is the current canonical mapping, but the plan was not updated alongside
it.

**Fix:** append a section (or amend the existing table) with the post-v3 row
mapping:

| Post-v1 target | Post-v3 target | Role              |
| -------------- | -------------- | ----------------- |
| `766a1b7`      | `8325bb8`      | P3                |
| `f08b841`      | `2a2d672`      | R1                |
| `b1bd180`      | `d4d0478`      | R2                |
| `331cbae`      | `534231b`      | R3                |
| `b14b413`      | `70def66`      | plan-progress doc |

### G12 — LOW: Plan's "R3 bundled change (F15)" section contradicts current commit message

`plans/017-20260419T2123-robot-accounts.md` lines 874-882 say:

> Commit 8 (`331cbae`, R3) also loosened `SetDefaultChannelBody.channel_id` from
> `i64` to `Option<i64>` so `cbc admin robots default-channel clear` could omit
> the field. … **but the R3 commit message did not call the server-side change
> out.**

The current R3 commit (`534231b`) message in fact **does** call this out:

```
The server's default-channel endpoint is extended to accept null so the
CLI can clear a robot's channel assignment.
```

The plan note is therefore incorrect as of branch tip. Either the commit message
was revised in the autosquash wave after F15 was first flagged, or the plan text
was written against an earlier draft. Either way it should be reconciled.

**Fix:** amend the plan section to say the loosening is called out in the R3
commit message and F15 is resolved.

### G13 — MICRO: `HUMAN_ONLY_ROUTES` `auth.rs` rationale omits two reachable handlers

The new `auth.rs` rationale (`audit_identity_lint.rs:33-39`) enumerates
`/token/revoke`, `/tokens/revoke-all`, and `/api-keys` as the handlers with
`user.email` log lines that reject non-humans upfront. Two more handlers in
`auth.rs` also log `user.email` inside `tracing::` macros:

- `list_api_keys_handler` (`routes/auth.rs:572-601`) — logs `user.email` at line
  `:586` (error-path) inside a `tracing::error!` macro. Robots are rejected by
  the `has_cap("apikeys:create:own")` check at `:576` (returns 403) because the
  cap is in `ROBOT_FORBIDDEN_CAPS` and stripped on every auth. Rejection is
  cap-based, not `is_robot`-explicit.
- `revoke_api_key_handler` (`routes/auth.rs:607-638`) — logs `user.email` at
  `:637` (`tracing::info!`). Same cap-based upfront rejection pattern.

The safety property ("robots can never reach the log line") holds via the
forbidden-cap strip, but the rationale text focuses on the three handlers with
explicit `is_robot` early-returns and is silent on these two. A future
contributor reading the comment may conclude the handlers without explicit
`is_robot` checks are unsafe to add `user.email` logs to, or conversely may add
a new handler without checking which of the two patterns applies.

**Fix:** extend the rationale sentence to "… and `/api-keys/*` handlers reject
robot callers via the `apikeys:create:own` cap, which is stripped for robots by
the forbidden-cap filter in `load_authed_user`." This documents both rejection
patterns.

### G14 — CARRY-OVER: `routes/channels.rs` retains local `is_unique_violation` copy

Unchanged from v3 (`routes/channels.rs:650-658`). The comment is now accurate
(cites `db::is_unique_violation` as the shared helper, see verification above);
the duplication itself persists because extracting the helper targets a
pre-robot-accounts-phase commit. Plan explicitly defers to a future
opportunistic cleanup.

This was already deducted at v3; re-deducting once to maintain score continuity
with the v3 anchor.

## Remaining Concerns from the Brief

| Concern                                              | Verdict at tip    | Evidence                                                                                                                                                                                                                                                                                                                 |
| ---------------------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| G7 regression coverage actually exercises the branch | OK                | See "G7 regression coverage — quality check" above.                                                                                                                                                                                                                                                                      |
| Bisectability on 6 specified commits                 | OK                | All six pass `cargo check --all-targets` (see Per-Commit Bisectability table). R1 emits a dead-code warning but is not broken. The v3 G9 finding is fully resolved.                                                                                                                                                      |
| Per-commit tests at R1 / R2 / R3                     | OK                | See per-commit test runs table. All three tips pass `cargo test --workspace` with 0 failures.                                                                                                                                                                                                                            |
| Audit lint false-positive surface                    | Acceptable        | Heuristic has four known edge cases (comments in macro body, string literals with unbalanced parens, same-line code after `tracing::*!()` call, `//` comments containing `user.email`). Not hit in current code. Acceptable given module's zero-tolerance policy; documenting the gaps inline would be a nice-to-have.   |
| Design v4 freshness                                  | OK                | `git log 79f26d1^..HEAD -- design/017-...-robot-accounts.md` shows the design was added at `79f26d1` and never touched afterwards. The revision-history header reads "Status: Draft v4" and the v4 entry is dated 2026-04-20, matching the brief.                                                                        |
| Plan progress tables                                 | PARTIAL (G11,G12) | `Progress`, `Review v1 Follow-ups`, `Review v2 Follow-ups`, `Autosquash SHA refresh`, and `R3 bundled change (F15)` sections all exist. `Autosquash SHA refresh` and `Review v2 Follow-ups` reference post-v1 SHAs, not current. `R3 bundled change` asserts a condition (commit message silent) that is no longer true. |
| channels.rs local `is_unique_violation` remains      | OK (intentional)  | Present at `routes/channels.rs:653-659`. Comment now accurate.                                                                                                                                                                                                                                                           |

## Plan Coverage Matrix (unchanged from v3 unless noted)

Every plan item's status at branch tip. Changes from v3 in **bold**.

### R1 — robot provision + `cbrk_` auth (commit `2a2d672`)

| Plan item                                                  | Status at tip |
| ---------------------------------------------------------- | ------------- |
| Migration `007_robot_accounts.sql`                         | OK            |
| `robot_tokens` table + partial unique index                | OK            |
| `count_active_wildcard_holders` gains `AND u.is_robot = 0` | OK            |
| `KNOWN_CAPS` gains `robots:manage`, `robots:view`          | OK            |
| Forbidden-cap strip in `load_authed_user`                  | OK-test       |
| Assignment-time forbidden-cap reject                       | OK-test       |
| SSO rejects name starting with `robot:`                    | OK-test       |
| `TokenCache` rename, `TokenKind { ApiKey, RobotToken }`    | OK            |
| `cbrk_` bearer dispatch in extractor                       | OK            |
| Argon2 timing parity on empty-candidate path               | OK-test       |
| Robot create / list / get / tombstone                      | OK-test       |
| `display_identity()` on builds.rs                          | OK            |
| Lint-style test                                            | OK-test       |
| **Revoke-self coexistence for `cbrk_`**                    | **OK-test**   |

R1 is now complete on every checklist item; the v3-flagged gap (revoke-self for
`cbrk_`) closed as part of G7.

### R2 — rotation + coexistence (commit `d4d0478`)

| Plan item                                                              | Status at tip |
| ---------------------------------------------------------------------- | ------------- |
| `rotate_token` (BEGIN IMMEDIATE, re-read, classify)                    | OK-test       |
| `POST /api/admin/robots/{name}/token` with `renew` body flag           | OK-test       |
| `DELETE /api/admin/robots/{name}/token` idempotent + 404               | OK-test       |
| `PUT /api/admin/robots/{name}/description` + 404                       | OK-test       |
| `list_entities_filtered` + `?type=user\|robot\|all`                    | OK-test       |
| `?type=garbage` → 400                                                  | OK-test       |
| `prefix_template_contains_username`                                    | OK-test       |
| `set_entity_default_channel` rejects robot + `${username}`             | OK            |
| `submit_build` rejects robot + `${username}` resolved channel          | OK            |
| `POST /api/auth/api-keys` rejects robot caller (400)                   | OK-test       |
| `POST /api/auth/tokens/revoke-all` rejects robot caller (400)          | OK-test       |
| **`POST /api/auth/token/revoke` rejects `cbrk_` bearer (self-revoke)** | **OK-test**   |

R2 also now complete — the v3-flagged gap moved into the auth route file and is
covered by the `revoke_token_rejects_cbrk_bearer_with_400` test added in the
v3→v4 fix wave.

### R3 — `cbc admin robots` (commit `534231b`)

Unchanged from v3. All checklist items OK except the "manual smoke workflow"
item which was marked "Not run" in v3 and remains so at v4. This is an
operator-validation step, not a regression.

### Non-code deliverables

| Item                                               | Status at tip   |
| -------------------------------------------------- | --------------- |
| Design v4 file present & untouched since bootstrap | OK              |
| v1 + v2 + v3 impl review files present             | OK              |
| Plan progress table marks everything "Done\*"      | OK              |
| Plan SHA-refresh tables reflect post-v3 SHAs       | **STALE (G11)** |
| Plan F15 section matches R3 commit message         | **STALE (G12)** |

## Commit Granularity Evaluation

Applying the `git-commits` skill (400-800 authored LOC target):

| SHA       | Commit                    | Authored LOC | Compiles alone                 | Verdict                 |
| --------- | ------------------------- | ------------ | ------------------------------ | ----------------------- |
| `5d51cf9` | P1                        | ~200         | yes                            | PASS                    |
| `ff349b3` | P2-guard                  | ~50          | yes                            | PASS                    |
| `be5afca` | P2 path reshape           | ~800         | yes                            | PASS                    |
| `8325bb8` | P3 token usage            | ~150         | yes                            | PASS                    |
| `2a2d672` | R1 robot provision + auth | ~2900        | yes (1 dead-code warning, G10) | MARGINAL                |
| `d4d0478` | R2 rotation + coexistence | ~1100        | yes                            | MARGINAL (above target) |
| `534231b` | R3 cbc admin robots       | ~1100        | yes                            | MARGINAL (above target) |

All commits carry DCO sign-off and the `Co-authored-by:` trailer. Commit message
style matches Ceph convention. R3's message now explicitly calls out the
server-side default-channel null loosening, closing the v2 F15 gap at the
commit-message layer (the plan text G12 is the remaining inconsistency).

R1's size is driven by the `api_keys.rs` → `token_cache.rs` rename (pure churn).
The authored robot-feature delta is closer to ~1200 lines. A split at the rename
boundary was possible in principle but would have made the R1 diff harder to
review as a cohesive feature — the plan's sizing-note acknowledgement (plan §
Sizing Notes) accepts this.

## Confidence Score

Starting at 100 and applying the `confidence-scoring` deduction table. Each
distinct finding is scored independently. Anchored against the v3 score of
88/100: v3-closed findings return points; new findings deduct them.

| Deduction                                       | Finding                                                                                     | Points  |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------- | ------- |
| D12 Commit boundary violation                   | G10 — R1 pre-extracts pool wrapper with no R1 caller; emits `#[warn(dead_code)]`            | −5      |
| D11 Missing / stale documentation               | G11 — plan's post-v1 SHA table and v2-follow-ups table are stale vs. post-v3 branch tip     | −3      |
| D11 Missing / stale documentation               | G12 — plan's F15 section contradicts the current R3 commit message                          | −2      |
| D10 Convention violation (incomplete rationale) | G13 — `auth.rs` lint rationale omits `list_api_keys` / `revoke_api_key` cap-based rejection | −1      |
| D2 Duplicated code (carry-over)                 | G14 — `routes/channels.rs` local `is_unique_violation` (explicitly deferred by plan)        | −1      |
| **Total**                                       |                                                                                             | **−12** |

**Score: 88 / 100**

### Delta from v3 (88 → 88)

The v3 fixup wave closed:

- G7 (−5) — `cbrk_` self-revoke branch + two handler tests landed
- G8 (−2) — `HUMAN_ONLY_ROUTES` rationale for `auth.rs` rewritten
- G9 (−2) — R1 `--all-targets` now clean (test module moved to R2, sqlx cache
  entry moved to R1)
- G7 test gap (−1) — tests exercise real `HeaderMap` + handler body
- channels.rs stale comment (−1) — comment now cites `db::is_unique_violation`

Total closed: 11 points.

The v3 fixup wave introduced / surfaced:

- G10 (−5) — R1 dead-code warning for `revoke_all_active_tokens`
- G11 (−3) — plan's SHA refresh table stale (not revised as part of the v3 wave)
- G12 (−2) — plan's F15 note now out-of-sync with the R3 commit message
- G13 (−1) — new `auth.rs` rationale incomplete for two more handlers
- G14 (−1) — carry-over `routes/channels.rs` duplicate (scored once to preserve
  v3 anchor)

Total new deductions: 12 points.

Net ≈ 0. The fix wave successfully closed the v3 findings but introduced a
similarly-sized set of smaller paperwork issues. The code-level quality trend is
clearly upward: every code-visible finding (G7, G8, G9 part (a), G9 part (b))
was closed; every new finding is either documentation drift (G11, G12) or a
lint-rationale / dead-code / carry-over duplication concern at the margin.

## Go / No-Go Recommendation

**Go.** The robot-accounts feature is production-ready: provisioning,
authentication (SSO + API-key + `cbrk_`), rotation, tombstone/revive lifecycles,
coexistence guards, and the `cbc admin robots` surface are all implemented,
tested, and bisectable. No finding rises to MODERATE or CRITICAL severity. The
v3 G7 operator-confusion footgun is closed and regression-guarded by the two new
handler tests.

### Required before merge (none)

No blockers. The following follow-ups are optional and may land as a small
post-merge cleanup commit:

### Follow-ups (ordered by priority)

1. **G11 (LOW, doc)** — Amend the plan's "Autosquash SHA refresh" and "Review v2
   Follow-ups" tables with the post-v3 SHAs. Single-table edit; no code changes.
   Touches only the plan doc.
2. **G12 (LOW, doc)** — Reconcile the plan's "R3 bundled change (F15)" section
   with the current R3 commit message. Either delete the note (F15 is resolved)
   or rewrite it to reflect that the loosening is now in the message body.
3. **G13 (MICRO)** — Extend the `HUMAN_ONLY_ROUTES` `auth.rs` rationale to
   enumerate the cap-based-rejection handlers (`list_api_keys`,
   `revoke_api_key`). One-line edit. Strengthens regression-guard clarity.
4. **G10 (LOW)** — Optionally fold `revoke_all_active_tokens` pool wrapper into
   R2 via another autosquash, removing the R1 dead-code warning. Alternatively
   accept the warning (no CI gate enforces it). Given the cost of a third rebase
   pass, accepting is reasonable.
5. **G14 (cosmetic, carry-over)** — `routes/channels.rs` local
   `is_unique_violation` copy. Plan explicitly defers; remove when
   opportunistically touched.
6. **P3 warn-on-failure test (deferred from v2)** — Remains rational to defer.
   Revisit only if another phase adds a `tracing-test` dev-dep.

## Summary (Severity-Ordered)

1. **G10 — LOW** — R1 (`2a2d672`) introduces `revoke_all_active_tokens` pool
   wrapper with no caller at R1. `cargo check --all-targets` emits
   `#[warn(dead_code)]` at R1. First caller lands in R2. No CI gate enforces
   warnings-as-errors so this is informational only, but it violates the
   `git-commits` smell test #5 (no dead code in a commit).
2. **G11 — LOW** — Plan's "Autosquash SHA refresh" table (lines 863-869) and
   "Review v2 Follow-ups" table (lines 890-900) still reference post-v1 SHAs
   that no longer exist on the branch. Navigational footgun for readers using
   the plan as an index.
3. **G12 — LOW** — Plan's "R3 bundled change (F15)" section (lines 874-882)
   asserts the R3 commit message is silent on the server-side default-channel
   null loosening. Current R3 commit message (`534231b`) explicitly mentions it.
   Stale.
4. **G13 — MICRO** — `HUMAN_ONLY_ROUTES` `auth.rs` rationale enumerates three
   handlers with explicit `is_robot` rejection but omits two more
   (`list_api_keys`, `revoke_api_key`) that log `user.email` and rely on the
   forbidden-cap strip of `apikeys:create:own`. Underlying safety holds;
   rationale text is incomplete.
5. **G14 — CARRY-OVER (cosmetic)** — `routes/channels.rs:650-658` local
   `is_unique_violation` duplicate remains. Comment now accurately cites
   `db::is_unique_violation` as the shared helper. Plan explicitly defers.
6. **G7, G8, G9 — RESOLVED** — Every v3 finding closed with code changes and
   (where applicable) regression tests. The two new handler tests
   (`revoke_token_rejects_cbrk_bearer_with_400`,
   `revoke_token_rejects_cbsk_bearer_with_400`) are high-quality: they exercise
   the real `HeaderMap` + `AuthUser` + handler body path, not a synthesized
   shortcut.

**Confidence score: 88 / 100.** Recommendation: **go**. The feature is
production-ready. All five follow-ups are documentation and cosmetic; none block
merging to `main`.
