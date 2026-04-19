<!-- Copyright (C) 2026  Clyso -->
<!--                                                              -->
<!-- This program is free software: you can redistribute it       -->
<!-- and/or modify it under the terms of the GNU Affero General   -->
<!-- Public License as published by the Free Software Foundation, -->
<!-- either version 3 of the License, or (at your option) any    -->
<!-- later version.                                               -->

# Review: Design 017 — Robot Accounts (v2)

**Document reviewed:**
`cbsd-rs/docs/cbsd-rs/design/017-20260417T1130-robot-accounts.md` (v2, dated
2026-04-19)

**Previous review:**
`cbsd-rs/docs/cbsd-rs/reviews/017-20260417T1841-design-robot-accounts-v1.md`

**Review type:** Design review (pre-implementation)

**Reviewer:** Code review pass — all claims verified against codebase

**Date:** 2026-04-19

---

## Review Summary

v2 is a substantial improvement. All four v1 blockers are resolved: direct caps
are eliminated (B1), deletion is replaced with tombstone semantics that respect
the `builds.user_email` FK (B2), the token cache is unified into a typed
`TokenCache` with a `kind` discriminator (B3), and the unimplementable duration
grammar is replaced with a calendar-date or `"infinity"` string (B4). All three
v1 Important findings are addressed: the coexistence table is now complete (I1),
build responses gain `is_robot: bool` alongside the raw `user_email` (I2), and
the last-admin guard gains `AND u.is_robot = 0` in P2 (I3).

No new blocking issues are found. Six new Important findings require resolution
before the implementation plan is written. The most consequential are: the dual
last-admin guard implementations (NF11), the `${username}` channel template
expansion for robot emails (NF5), and the unspecified transaction isolation
level for the revive path (NF2).

**Go/No-Go: Go with conditions.** The design is approved for implementation plan
authoring once the six Important findings below are resolved in the design
document. No second full review pass is required; the plan author may resolve
findings inline during plan writing.

---

## Verification Method

All claims were checked against the following source files:

- `cbsd-rs/migrations/001_initial_schema.sql`
- `cbsd-rs/cbsd-server/src/auth/extractors.rs`
- `cbsd-rs/cbsd-server/src/auth/api_keys.rs`
- `cbsd-rs/cbsd-server/src/app.rs`
- `cbsd-rs/cbsd-server/src/routes/admin.rs`
- `cbsd-rs/cbsd-server/src/routes/permissions.rs`
- `cbsd-rs/cbsd-server/src/routes/auth.rs`
- `cbsd-rs/cbsd-server/src/db/roles.rs`
- `cbsd-rs/cbsd-server/src/db/users.rs`
- `cbsd-rs/cbsd-server/src/channels/mod.rs`
- `cbsd-rs/cbc/src/admin/users.rs`
- `cbsd-rs/cbc/src/admin/roles.rs`
- `cbsd-rs/docs/cbsd-rs/design/003-20260313T2129-cbsd-auth-permissions-design.md`
- `cbsd-rs/docs/cbsd-rs/design/016-20260402T1200-role-level-scopes.md`

---

## Part 1: v1 Regression Check

### B1 — Direct caps break the AND-scope invariant

**Resolution: Resolved.** `robot_direct_caps` is eliminated entirely. All robot
permissions flow through role assignments, keeping `require_scopes_all` and
`get_user_assignments_with_scopes` unchanged. The design explicitly documents
the rationale (preserves the confused-deputy protection from design 003).

### B2 — `builds.user_email` FK blocks deletion

**Resolution: Resolved.** Deletion is replaced with tombstone semantics:
`users.active = 0`, all `robot_tokens` rows revoked, but the `users` row is
preserved. The synthetic email satisfies the FK for all historical builds. No
migration to `builds` is required. This is the correct approach.

### B3 — LRU cache integration unspecified

**Resolution: Resolved.** The design specifies unifying `ApiKeyCache` into
`TokenCache`, renaming `CachedApiKey` to `CachedToken`, and adding
`kind: TokenKind { ApiKey, RobotToken }` as a discriminator.
`AppState.api_key_cache` is renamed `token_cache`. Invalidation methods
(`remove_by_owner`, `remove_by_prefix`) work uniformly for both token types.

### B4 — Duration string parsing unimplementable

**Resolution: Resolved.** The design replaces the unimplementable
`humantime`/`chrono` grammar with a calendar-date string
`YYYY-MM-DD | "infinity"`. Duration arithmetic is explicitly deferred to a
follow-on change. This is clean and implementable.

### I1 — Coexistence table incomplete

**Resolution: Resolved.** The coexistence table in v2 covers all three
previously missing endpoints: `POST /api/auth/api-keys` (400 if target
`is_robot = 1`), `GET /api/auth/whoami` (gains `is_robot: bool`), and the
missing `default-channel` endpoint is now subsumed under the P2 entity refactor
which handles it uniformly for both row types.

### I2 — Build response `user_email` for robots

**Resolution: Resolved.** v2 specifies that build response bodies gain
`is_robot: bool` alongside `user_email`. The raw synthetic email is returned in
`user_email`; callers distinguish via `is_robot`.

### I3 — `last_admin_guard` does not filter robots

**Resolution: Resolved.** Design explicitly adds `AND u.is_robot = 0` to
`count_active_wildcard_holders` in P2, with correct defense-in- depth rationale.

### S1–S4 — Suggestions

All resolved. `idx_robot_tokens_robot` and `idx_robot_tokens_prefix` are
specified in Migration 2. `token_status` metadata is embedded in
`GET /api/admin/robots/{name}`. Concurrent `token new` is handled by the partial
unique index (`idx_robot_tokens_active`) rejecting a second insert rather than a
check-then-insert race. Cache invalidation is specified in the tombstone,
revive, and rotation sequences.

### S5 — Zero-cap baseline

**Partially resolved.** The design correctly states that all robot permissions
flow from roles and that a robot with no roles has an empty effective cap set.
This is now derivable from the design, but is not stated as an explicit
invariant. See suggestion NF9 below.

---

## Part 2: New Findings

### NF1 — `YYYY-MM-DD` expiry: timezone semantics need a doc fix

**Severity: Important**

The design states: `"2026-12-31"` → `expires_at = 1767225600` (epoch for
`2027-01-01T00:00:00Z`). The prose says "valid through the end of that UTC day;
stored as `00:00:00 UTC` on the day **after** the given date." This is
internally consistent.

However, the API surface exposes a date string without a timezone qualifier. An
operator in UTC+8 who writes `--expires 2026-12-31` expecting the token to be
valid through their local midnight will find the token expiring eight hours
sooner than expected. This is not a bug (UTC is unambiguous once documented) but
the documentation gap will cause support tickets.

**Recommendation:** Add a single sentence to the Expiry section and to the
`cbc admin robots token new` help text: "Dates are interpreted as UTC.
`2026-12-31` means the token expires at `2027-01-01 00:00:00 UTC`."

---

### NF2 — Revive transaction isolation not specified

**Severity: Important**

The revive path (the six-step transaction in "Create or Revive Robot") executes
six writes in a single transaction. The design does not specify the isolation
level. SQLite supports `BEGIN DEFERRED` (default), `BEGIN IMMEDIATE`, and
`BEGIN EXCLUSIVE`.

With `BEGIN DEFERRED`, two concurrent revive requests for the same tombstoned
robot both acquire a read lock, both read `active = 0`, both proceed through
steps 1–4, and then both attempt to insert into `robot_tokens`. One will win;
the other will fail with a constraint error. The constraint error is on
`token_hash` (UNIQUE), not on `robot_email`, so the error is not easily mapped
to a `409 Conflict` — it looks like a generic `500` unless explicitly handled.

With `BEGIN IMMEDIATE`, the second revive is blocked at lock acquisition, not at
the insert. The first commits; the second then reads `active = 1` and returns
`409 Conflict` via the "Active robot" branch. This is the correct user-visible
behaviour.

**Recommendation:** Specify `BEGIN IMMEDIATE` for the revive transaction in the
design. Add a note that the same requirement applies to the tombstone
transaction (the active-to-tombstone path also reads-then-writes on `users`).

---

### NF3 — `POST /api/auth/tokens/revoke-all` silently no-ops for robot emails

**Severity: Important**

`routes/auth.rs` has `POST /api/auth/tokens/revoke-all` which writes:

```sql
UPDATE tokens SET revoked = 1
WHERE user_email = $1 AND revoked = 0
```

This operates on the `tokens` table (PASETO sessions), not on `robot_tokens`. If
a caller somehow presents a robot bearer token and calls this endpoint, it will
succeed with `200 OK` and a message that implies all tokens were revoked — but
the robot token in `robot_tokens` is untouched.

The endpoint is nominally self-service (it operates on the authenticated user's
own email), and robots do not hold PASETO sessions. However, the response is
misleading. An admin using a robot token who is also monitoring human session
revocations may see a false success.

The design's coexistence table does not address this endpoint.

**Recommendation:** Add `POST /api/auth/tokens/revoke-all` to the coexistence
table with action: "Return `400 Bad Request` (or `403 Forbidden`) when the
authenticated caller has `is_robot = true`; robots have no PASETO sessions."

---

### NF4 — Forbidden-cap check at role-assignment time: no warning on role update

**Severity: Important**

The design specifies two enforcement points for forbidden caps:

1. At auth time (strip forbidden caps from the verified set).
2. At role-assignment time (reject assignment of a role containing forbidden
   caps to a robot target).

This covers the creation and assignment flows. However, there is a third
mutation path: a role that was valid at assignment time is later updated by an
admin to include a forbidden cap. The existing role update endpoint
(`PUT /api/permissions/roles/{name}/caps`) does not know which entities hold
that role, and the design does not specify any re-validation on role update.

The auth-time strip is the primary guard and is correct. But the design does not
acknowledge this gap, which means an operator inspecting
`GET /api/admin/robots/{name}` may see `effective_caps` containing a forbidden
cap — the stored result of a role-update before the next auth event strips it.

This is not a security gap (auth-time strip happens before any permission
check), but it is an observability gap and a potential source of operator
confusion.

**Recommendation:** Add a note to the Permissions Model section: "When a role is
updated to include a forbidden cap and that role is assigned to a robot, the
forbidden cap will appear in the role's `role_caps` row but will be absent from
the robot's computed `effective_caps` at next auth.
`GET /api/admin/robots/{name}` computes `effective_caps` through the same
stripping logic as auth, so the response is always accurate."

---

### NF5 — `${username}` channel template expands to `robot+<name>` for robots

**Severity: Important**

`cbsd-rs/cbsd-server/src/channels/mod.rs` resolves the `${username}` template
variable by taking the substring before `@` in the user's email:

```rust
fn resolve_prefix_template(email: &str, ...) -> String {
    // takes everything before '@'
    let username = email.split('@').next().unwrap_or(email);
    ...
}
```

For a robot with synthetic email `robot+ci-builder@robots`, this produces
`${username}` → `robot+ci-builder`. If the default channel for a robot is set to
a channel whose prefix template contains `${username}`, the resolved prefix will
include the `robot+` sentinel, resulting in image paths like
`registry/robot+ci-builder/image:tag`. This is almost certainly undesirable.

The design does not address channel template behaviour for robots.

**Recommendation:** Either:

- Strip the `robot+` prefix before expanding `${username}` for robots:
  `robot+ci-builder@robots` → `${username}` = `ci-builder`. Implement via the
  `is_robot` flag on `AuthUser`.
- Document that robots should not use channels with `${username}` templates and
  that `default_channel_id` should be set to a channel with a static prefix.
  State this as a constraint in the design.

The first option is safer and should be the default. Add a test case:
`resolve_prefix_template("robot+ci-builder@robots", ..., is_robot=true)` →
`ci-builder`.

---

### NF6 — Entity deactivate handler: robot vs. human treatment unspecified

**Severity: Important**

The P2 entity refactor moves `PUT /api/admin/users/{email}/deactivate` and
`PUT /api/admin/users/{email}/activate` to
`PUT /api/admin/entity/{email}/deactivate` and
`PUT /api/admin/entity/{email}/activate`.

For a human user, deactivate sets `users.active = 0` and revokes all PASETO
tokens. For a robot, setting `users.active = 0` is a disable (leaves
`robot_tokens` intact, reversible) not a tombstone (revokes all `robot_tokens`,
also sets `active = 0`).

The design does not specify what `PUT /api/admin/entity/{email}/deactivate` does
to the `robot_tokens` rows. If the handler is naively ported from the human
path, it will leave active robot tokens in place — meaning the robot can still
authenticate after an admin calls "deactivate." This is a security gap: an admin
who expects deactivation to cut off access immediately gets incorrect behaviour
for robots.

Note: the current `deactivate_user` handler in `routes/admin.rs` (lines 98–111)
also has its own inline last-admin guard SQL that is NOT a call to
`db::roles::count_active_wildcard_holders`. The P2 migration note only mentions
updating the DB function — this inline copy will not be updated unless
explicitly called out.

**Recommendation:**

1. Add to the P2 section: "The entity deactivate handler must revoke all active
   `robot_tokens` rows when `is_robot = 1`, in addition to setting
   `active = 0`." This aligns with the disable/revoke/ tombstone semantics
   table.
2. Add a checklist item to P2: "Resolve the dual last-admin guard
   implementations — the inline SQL in the current `deactivate_user` handler
   must be replaced with a call to `count_active_wildcard_holders` so that the
   `is_robot = 0` filter takes effect everywhere."

---

### NF7 — `cbc admin robots revive` is CLI-level redundancy, not a gap

**Severity: Suggestion**

The design adds `cbc admin robots revive <name>` as "a convenience wrapper for
`cbc admin robots create <name>` when the name is known to be tombstoned." Both
call `POST /api/admin/robots` with the same body. The server determines
create-vs-revive based on DB state.

The duplication is minor and the design documents it accurately. The only risk
is operator confusion: "why did `revive` fail?" when the answer is "because the
name isn't tombstoned" — the same 409 that `create` would produce for a live
robot. Since the error message from the server is the same for both, this is
acceptable.

**Recommendation (optional):** Consider adding a note: "`revive` returns 409 if
the account is currently active (same as `create` on a live name). It is a
client-side naming convenience, not a distinct server operation."

---

### NF8 — Token rotation: `token new --renew` on expired token needs a test

**Severity: Suggestion**

The `token new` semantics table shows:

| Current token state | `token new` | `token new --renew` |
| ------------------- | ----------- | ------------------- |
| Active or expired   | **Refuse**  | Rotate              |

The rotation transaction (step 1: `revoked = 1` on the old row, then insert new
row) targets "the non-revoked row." When the token is expired (`revoked = 0` but
`expires_at` is in the past), there is still a non-revoked row. The transaction
is correct.

However, this path is easy to forget in tests: an expired-but-not- revoked token
is a distinct state from "only revoked rows exist." The implementation checklist
should call out a test for `token new` on a robot with an expired (non-revoked)
token.

**Recommendation:** Add to the implementation checklist: "Test `token new`
without `--renew` on a robot with an expired (non- revoked) token — must return
409, not create a new token."

---

### NF9 — Zero-cap baseline: state it as an explicit invariant

**Severity: Suggestion**

v1 S5 asked for explicit confirmation that a robot with no roles has an empty
effective cap set and all requests are rejected. v2 implies this but does not
state it as an invariant.

**Recommendation:** Add to the Permissions Model section: "A robot with no role
assignments has an empty effective cap set. All permission checks fail. This is
the correct zero-trust baseline and requires no special handling."

---

## Confidence Score

| Item                                                               | Points  | Description                                                                                                 |
| ------------------------------------------------------------------ | ------- | ----------------------------------------------------------------------------------------------------------- |
| Starting score                                                     | 100     |                                                                                                             |
| D8: NF1 — timezone not documented for API consumers                | -5      | `YYYY-MM-DD` expiry is UTC but not stated; operators in non-UTC timezones will be surprised                 |
| D8: NF2 — revive transaction isolation not specified               | -5      | Missing `BEGIN IMMEDIATE` spec; concurrent revives produce an unmapped constraint error                     |
| D8: NF3 — `tokens/revoke-all` silently no-ops for robot callers    | -5      | Not in coexistence table; misleading 200 OK for a robot calling the endpoint                                |
| D11: NF4 — role-update/forbidden-cap gap not documented            | -5      | `effective_caps` in robot detail response may confuse operators after a role update                         |
| D7: NF5 — `${username}` expands to `robot+<name>` for robot emails | -5      | Image paths include `robot+` sentinel; wrong channel prefix for all robot builds using username templates   |
| D7: NF6 — entity deactivate handler does not revoke robot tokens   | -5      | Robot can still authenticate after admin deactivate; security regression vs. documented semantics           |
| D10: NF6 (second part) — dual last-admin guard not addressed       | -5      | P2 migration note only updates `count_active_wildcard_holders`; inline guard in `deactivate_user` uncovered |
| D11: NF8 — expired token rotation path not in test checklist       | -3      | Easy-to-miss edge case for `token new --renew` on expired token                                             |
| D11: NF9 — zero-cap baseline not stated as invariant               | -2      | S5 from v1 only partially resolved                                                                          |
| **Total deductions**                                               | **-40** |                                                                                                             |
| **Final score**                                                    | **60**  |                                                                                                             |

---

## Interpretation

| Range     | Meaning                                                    |
| --------- | ---------------------------------------------------------- |
| 90–100    | Ready to merge. Minor or no issues.                        |
| 75–89     | Acceptable with noted improvements. Fix before next stage. |
| **50–74** | **Significant issues. Must address before proceeding.**    |
| 0–49      | Major rework needed. Block merge.                          |

Score: **60 / 100 — Significant issues. Resolve Important findings (NF1–NF6)
before writing the implementation plan.**

---

## Top-3 Blocking Findings

1. **NF6 — Entity deactivate handler must revoke robot tokens, and the dual
   last-admin guard must be unified.** The P2 handler migration is incomplete as
   specified: a naively ported `deactivate_user` leaves active robot tokens in
   place (robot remains authenticated after admin deactivate), and the inline
   guard SQL in the current `deactivate_user` handler bypasses the
   `is_robot = 0` filter that P2 adds to `count_active_wildcard_holders`.

2. **NF5 — `${username}` channel template expansion for robot emails.** All
   robot builds using a channel with a `${username}` prefix template will
   produce image paths containing `robot+<name>` instead of `<name>`. This is a
   silent data-correctness bug that will only surface when builds are inspected
   post-implementation.

3. **NF2 — Revive transaction must specify `BEGIN IMMEDIATE`.** The six-step
   revive path does not specify isolation level. Concurrent revives will race at
   the `token_hash` UNIQUE constraint, producing an unmapped error that surfaces
   as a 500. `BEGIN IMMEDIATE` serialises concurrent revives before any writes,
   allowing the loser to fail cleanly with 409.
