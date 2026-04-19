<!-- Copyright (C) 2026  Clyso -->
<!--                                                              -->
<!-- This program is free software: you can redistribute it       -->
<!-- and/or modify it under the terms of the GNU Affero General   -->
<!-- Public License as published by the Free Software Foundation, -->
<!-- either version 3 of the License, or (at your option) any    -->
<!-- later version.                                               -->

# Review: Design 017 — Robot Accounts (v1)

**Document reviewed:**
`cbsd-rs/docs/cbsd-rs/design/017-20260417T1130-robot-accounts.md`

**Review type:** Design review (pre-implementation)

**Reviewer:** Code review pass — all claims verified against codebase

**Date:** 2026-04-17

---

## Review Summary

The robot accounts design is architecturally sound at a high level: the identity
model (synthetic email in `users`), token format (`cbrk_` prefix, argon2id
hash), RBAC integration (additive union, forbidden caps), and storage shape are
all reasonable. However, four blocking issues make the design unimplementable as
written. Two of these are security-class gaps; the other two would produce code
that cannot compile or cannot function correctly. An additional six non-blocking
issues require clarification before the implementation plan is written.

**Go/No-Go: No-Go.** Design requires a v2 revision before an implementation plan
is produced.

---

## Verification Method

Every claim in the design was checked against one or more of the following
source files:

- `cbsd-rs/migrations/001_initial_schema.sql`
- `cbsd-rs/cbsd-server/src/auth/extractors.rs`
- `cbsd-rs/cbsd-server/src/auth/api_keys.rs`
- `cbsd-rs/cbsd-server/src/app.rs`
- `cbsd-rs/cbsd-server/src/routes/permissions.rs`
- `cbsd-rs/cbsd-server/src/routes/admin.rs`
- `cbsd-rs/cbsd-server/src/db/roles.rs`
- `cbsd-rs/cbc/src/admin/roles.rs`
- `cbsd-rs/cbc/src/admin/users.rs`
- `cbsd-rs/docs/cbsd-rs/design/003-20260313T2129-cbsd-auth-permissions-design.md`

---

## Area 1: Requirements Coverage

**Finding:** All stated goals have a named mechanism. The identity model, token
lifecycle, permission model, REST surface, and CLI surface are each addressed
somewhere in the document. No goal is left entirely without a mechanism.

**Gap:** Goal "robot cannot escalate via API key creation" maps to the
`apikeys:create:own` forbidden cap, which is correctly specified. But goal
"robot cannot be manipulated via human-user admin endpoints" maps to the
coexistence table — and that table is incomplete (see Area 6).

**Status:** Acceptable with noted gap in coexistence coverage.

---

## Area 2: Schema Correctness

### 2.1 `users` table — `is_robot` column

The design adds `is_robot INTEGER NOT NULL DEFAULT 0` to the `users` table.
Verified: the current `users` table in `migrations/001_initial_schema.sql` has
no such column. A new migration is required. The design correctly calls this
out.

### 2.2 `robot_tokens` table

The design specifies:

```sql
CREATE TABLE robot_tokens (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    robot_email TEXT NOT NULL REFERENCES users(email) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    expires_at  INTEGER,
    revoked     INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (robot_email, name)
);
```

This is internally consistent. The `ON DELETE CASCADE` on
`robot_email → users(email)` is correct and necessary.

### 2.3 BLOCKING — `builds.user_email` FK has no `ON DELETE` clause

The current schema has:

```sql
builds (
    ...
    user_email  TEXT NOT NULL REFERENCES users(email),
    ...
)
```

There is no `ON DELETE` clause. SQLite defaults to `NO ACTION` (equivalent to
`RESTRICT`). Deleting a `users` row that is referenced by any `builds` row will
fail at the FK constraint, provided `PRAGMA foreign_keys = ON` is set (which the
server does set per Correctness Invariant 3 in `cbsd-rs/CLAUDE.md`).

The design states that robot accounts can be permanently deleted and that
deletion revokes the robot token and removes the user row. This is
unimplementable as written: any robot that ever submitted a build cannot be
deleted.

**Required resolution (choose one):**

- **Option A (recommended):** Change the deletion semantics to "deactivate only"
  — set `users.active = 0`, revoke all tokens, but never delete the `users` row.
  Permanent deletion is not supported for robots with build history (same
  constraint applies to human users today).
- **Option B:** Add `ON DELETE SET NULL` to `builds.user_email` (requires making
  the column nullable and updating all queries that assume non-null).
- **Option C:** Add `ON DELETE CASCADE` to `builds.user_email`. This silently
  deletes build history, which is likely unacceptable.

This is a migration-level schema change. Option A requires no migration change;
the deletion endpoint simply returns `409 Conflict` with a message directing the
caller to deactivate instead.

### 2.4 `robot_direct_caps` table

The design proposes:

```sql
CREATE TABLE robot_direct_caps (
    robot_email TEXT NOT NULL REFERENCES users(email) ON DELETE CASCADE,
    cap         TEXT NOT NULL,
    PRIMARY KEY (robot_email, cap)
);
```

The table shape is correct for what the design describes. However, this table is
implicated in the blocking security gap documented in Area 3.

### 2.5 Indexes

The design does not specify indexes on `robot_tokens(robot_email)`,
`robot_tokens(token_hash)` (the `UNIQUE` constraint creates one implicitly), or
`robot_direct_caps(robot_email)`. The lookup path during authentication hashes
the presented token and queries `robot_tokens` by `token_hash`. The implicit
unique index on `token_hash` is sufficient for that path. An explicit index on
`robot_email` for "list tokens for robot" and "revoke all tokens on deletion" is
missing but not blocking.

**Suggestion:** Add
`CREATE INDEX idx_robot_tokens_email ON robot_tokens(robot_email)`.

---

## Area 3: Auth Flow Correctness

### 3.1 Bearer token dispatch

The current extractor in `cbsd-server/src/auth/extractors.rs` checks whether a
bearer token has the `cbsk_` prefix to route to the API key path. The design
adds a `cbrk_` prefix check that routes to a new robot token verification path.
This is the correct architectural pattern and consistent with the existing code.

### 3.2 BLOCKING — Direct caps break the single-assignment AND

scope invariant

Design 003 (authoritative) defines the scope enforcement model:
`require_scopes_all` iterates over `AssignmentWithScopes` records. Each record
represents a role assignment. For a cap to be granted **with** a scoped
operation, the scope must be present **in the same assignment record** as the
cap — AND semantics within one assignment, OR across assignments. This is the
confused-deputy protection.

The current implementation of `require_scopes_all` in `extractors.rs` queries
`get_user_assignments_with_scopes`, which returns rows from
`user_roles JOIN role_caps JOIN role_scopes`. There is no provision for
`robot_direct_caps` in this query.

When `builds:create` is granted via a direct cap in `robot_direct_caps` (not
through a role), the `AssignmentWithScopes` record for that cap does not exist.
The design does not specify:

1. Whether `require_scopes_all` is modified to include a synthetic assignment
   record for direct caps.
2. If so, what scopes are attached to that synthetic record.
3. Whether direct caps are treated as globally-scoped (all channels, all
   registries) — which reopens the confused-deputy vulnerability.

**The design cannot be implemented correctly without answering these
questions.** If direct caps are merged into a flat union with role-contributed
caps, the scope isolation guarantee documented in design 003 is broken for
robots.

**Recommended resolution:** Eliminate `robot_direct_caps` entirely. Require all
robot permissions — including those for robots with no role — to come from a
role. Provide a mechanism to create ephemeral roles or use a `--cap` flag on
`robot role assign` to create a named capability-only role. This keeps
`require_scopes_all` unchanged and preserves the security invariant.

### 3.3 `AuthUser.is_robot` field

The design adds `is_robot: bool` to `AuthUser`. Verified: the current `AuthUser`
struct in `extractors.rs` has `email`, `name`, and `caps`. The `is_robot` field
is not present and must be added. The design is correct that this field is
needed.

### 3.4 LRU cache integration — BLOCKING

The existing `ApiKeyCache` in `cbsd-server/src/auth/api_keys.rs` stores
`CachedApiKey`:

```rust
pub struct CachedApiKey {
    pub api_key_id: i64,
    pub owner_email: String,
    pub key_prefix: String,
    pub expires_at: Option<i64>,
}
```

The design states that robot tokens use an LRU cache that stores `AuthUser` as
the cached value. These are incompatible types.

The design does not specify:

- Whether a new `RobotTokenCache` is added alongside the existing `ApiKeyCache`
  in `AppState`.
- Whether the existing cache is extended to handle both types (would require a
  sum type or trait object).
- How cache invalidation on revocation (`remove_by_owner`, `remove_by_prefix`)
  applies to robot tokens.
- What the cache capacity is.

`AppState` in `app.rs` has `api_key_cache: Arc<Mutex<ApiKeyCache>>` but no robot
token cache field. The implementation plan cannot be written without this
decision.

**Recommended resolution:** Add a separate
`robot_token_cache: Arc<Mutex<RobotTokenCache>>` field to `AppState`, where
`RobotTokenCache` mirrors `ApiKeyCache` but stores the hash-to-`AuthUser`
mapping. Define the invalidation method used on token revocation/rotation.

---

## Area 4: Permissions Model

### 4.1 Additive union

The design correctly states that a robot's effective caps are the union of caps
from all assigned roles plus any direct caps. This is consistent with how human
user caps work.

### 4.2 Forbidden caps

The design specifies four forbidden caps that must be enforced at token
verification time:

- `*` (wildcard admin)
- `permissions:manage`
- `robots:manage`
- `apikeys:create:own`

This is the correct defense-in-depth layer. The verification path must check
these even if the role assignment was valid at creation time (a role may be
modified after assignment).

### 4.3 `KNOWN_CAPS` not updated

The constant `KNOWN_CAPS` in `cbsd-server/src/routes/permissions.rs` does not
include `robots:manage` or `robots:view`. The design says to add these. This is
not blocking but must be in the implementation checklist.

### 4.4 `last_admin_guard` does not filter robots

The `last_admin_guard` in `routes/permissions.rs` calls
`db::roles::count_active_wildcard_holders`. This counts users with the `*` cap.
If a robot somehow ends up with `*` (which the forbidden-cap check should
prevent), it would be counted as a wildcard holder and could incorrectly satisfy
the last-admin guard.

The defense-in-depth fix is to add `WHERE users.is_robot = 0` (or equivalent) to
`count_active_wildcard_holders`. The design does not mention this.

### 4.5 `robots:manage` vs `robots:view` split

The design defines `robots:manage` for write operations and `robots:view` for
read. This is consistent with the existing cap naming convention (`users:view`,
`users:manage`, etc.).

---

## Area 5: REST API Completeness

### 5.1 Endpoint coverage

The design specifies the following robot-specific endpoints:

```
POST   /api/robots                         create robot
GET    /api/robots                         list robots
GET    /api/robots/{name}                  get robot details
DELETE /api/robots/{name}                  delete robot
PUT    /api/robots/{name}/active           activate/deactivate
GET    /api/robots/{name}/roles            list role assignments
POST   /api/robots/{name}/roles            assign role
DELETE /api/robots/{name}/roles/{role}     remove role assignment
GET    /api/robots/{name}/caps             list direct caps
POST   /api/robots/{name}/caps             add direct cap
DELETE /api/robots/{name}/caps/{cap}       remove direct cap
POST   /api/robots/{name}/token            create/rotate token
DELETE /api/robots/{name}/token            revoke token
```

This surface is logically complete for the stated requirements.

### 5.2 Missing: token listing endpoint

There is no `GET /api/robots/{name}/token` (retrieve token metadata — not the
secret). This means callers cannot inspect token expiry, creation date, or name
without going through `GET /api/robots/{name}`. The design should clarify
whether token metadata is included in the robot detail response or exposed
separately.

### 5.3 Error cases: deletion with active builds

The design does not specify what `DELETE /api/robots/{name}` returns when the
robot has existing build records. Given the FK constraint finding (Area 2.3),
this endpoint will receive a DB error that must be mapped to a user-meaningful
HTTP response. The design must specify the status code and body for this case.

### 5.4 Error cases: token rotation concurrency

`POST /api/robots/{name}/token` without `--renew` must be transactional: check
that no active non-revoked token exists, then insert. The design only calls out
atomicity for the `--renew` (rotate) path. The check-then-insert for the initial
creation path must also be transactional (single statement with `INSERT OR FAIL`
or equivalent) to prevent duplicate tokens under concurrent requests.

---

## Area 6: CLI Completeness

### 6.1 CLI-to-REST parity

The design specifies a `cbc robot` subcommand tree that maps 1:1 to the REST
surface. No REST endpoint lacks a CLI counterpart.

### 6.2 `--yes-i-really-mean-it` flag prerequisite not established

The design requires `cbc robot delete` and `cbc robot token revoke` to accept
`--yes-i-really-mean-it` as a safety confirmation flag, and describes this as
following an "established pattern."

Verified: no existing destructive command in the `cbc` codebase uses this flag.
`cbc admin roles delete` uses `--force`. `cbc admin users deactivate` has no
confirmation flag at all. The `--yes-i-really- mean-it` pattern does not yet
exist.

This is not blocking — the flag can be introduced with the robot commands — but
the design should not describe it as "established." The implementation plan must
include adding the flag to these new commands without implying it exists
elsewhere.

### 6.3 `cbc robot token new --duration` parsing unimplementable

The design specifies a duration string grammar: `30d`, `6mo`, `1y`, `1y6mo`. The
design states this uses "`humantime` crate conventions extended with `mo`/`y`
support via `chrono`."

This is technically incorrect on two counts:

1. The `humantime` crate supports `s`, `m`, `min`, `ms`, `us`, `ns` but not `d`,
   `mo`, or `y`.
2. `chrono` has no duration-string parser. `chrono::Duration` is a type, not a
   parser.

No existing Rust crate in the workspace currently handles this grammar. The
design must specify either:

- A custom parser for the `30d`/`6mo`/`1y` grammar (straightforward to write;
  ~20 lines), or
- A different duration input format (e.g., ISO 8601 `P30D`, or separate
  `--days`/`--months`/`--years` flags).

This is a blocking gap: the implementation plan cannot reference a non-existent
library API.

**Recommended resolution:** Specify a small custom parser in `cbsd-proto` or a
utility module in `cbc`. The grammar is simple enough (`(\d+y)?(\d+mo)?(\d+d)?`)
that a bespoke parser is preferable to an external dependency.

---

## Area 7: Integration Points (Coexistence Table)

### 7.1 Verified correct entries

The design's coexistence table correctly identifies that:

- `GET /api/admin/users` must filter `is_robot = 0`
- `POST /api/admin/users/{email}/roles` must reject robot emails
- `DELETE /api/admin/users/{email}/roles/{role}` must reject robot emails
- `DELETE /api/admin/users/{email}` must reject robot emails (or be handled via
  the robot-specific delete path)

These are the correct integration points in `routes/admin.rs` and
`routes/permissions.rs`.

### 7.2 Missing: `PUT /api/admin/users/{email}/default-channel`

This endpoint exists in `cbsd-server/src/routes/admin.rs` (verified) and is not
in the coexistence table. It accepts an arbitrary `email` path parameter. If
called with a robot's synthetic email, it would set a `default_channel` on the
robot's user row (assuming the `users` table gains that column, which is
unrelated but possible in future). The design should either:

- Add this endpoint to the coexistence table with action "reject if
  `is_robot = 1`", or
- Confirm that the endpoint is safe to call with a robot email (no-op or
  ignored).

### 7.3 Missing: `POST /api/auth/api-keys`

An admin could attempt to create a standard API key for a robot's synthetic
email (e.g., via the admin-on-behalf path). The design says robots use robot
tokens, not API keys. The coexistence table should specify whether
`POST /api/auth/api-keys` rejects robot emails or permits dual-token types.

### 7.4 Missing: `GET /api/auth/whoami` for robot callers

The design does not specify what `GET /api/auth/whoami` returns when called by a
robot bearer token. The current response shape includes `email`, `name`, and
`roles`. For robots: should the synthetic email be returned as-is? Should
`is_robot: true` appear in the response? This is a user-facing behavior gap.

### 7.5 Missing: build response `user_email` for robot builds

`GET /api/builds/{id}` and the build list response include `user_email`. For
builds submitted by a robot, this field will contain the synthetic email
(`robot+<name>@robots`). The design does not specify whether this is
intentional, whether a display name should be derived, or whether an adjacent
`is_robot` field should appear. This affects API consumers parsing build
records.

---

## Area 8: Token Lifecycle

### 8.1 Active / expired / revoked / none — all four states handled

The design addresses:

- **Active:** token present, `revoked = 0`, `expires_at` either null or in the
  future.
- **Expired:** token present, `expires_at` in the past. Treated as invalid (same
  as revoked for auth purposes).
- **Revoked:** `revoked = 1`. Row retained as tombstone.
- **None:** no row in `robot_tokens`. `POST .../token` creates the first token.

The tombstone semantics (retain revoked rows, do not re-use names) are correctly
stated.

### 8.2 Rotation atomicity

The design specifies that `POST .../token` with `--renew` performs: (1) insert
new token, (2) revoke old token, in a single transaction. This is correct. The
implementation must use `BEGIN IMMEDIATE` or equivalent to prevent a window
where both tokens are simultaneously valid.

### 8.3 Initial creation race (see Area 5.4)

Covered under REST API completeness. The check-then-insert for initial token
creation must be transactional.

---

## Area 9: Edge Cases

### 9.1 Deletion with active token

The design states that deleting a robot revokes the token first, then deletes
the user row. With `ON DELETE CASCADE` on `robot_tokens.robot_email`, the token
row is deleted automatically when the user row is deleted. Explicit revocation
before deletion is redundant if deletion is the final state — but if the cache
is checked before the DB transaction commits, a cached valid token could be used
in the window between cache population and row deletion. The design must specify
cache invalidation as part of the deletion transaction/response path.

### 9.2 Role deletion cascade to robots

`user_roles` has `ON DELETE CASCADE` on both `user_email` and `role_name`.
Deleting a role automatically removes all assignments for that role, including
robot assignments. This is correct and no design intervention is needed. The
design correctly notes this.

### 9.3 Robot with no roles and no direct caps

A robot with no role assignments and no direct caps has an empty effective cap
set. Any request it makes will be rejected at the permission check. This is the
correct zero-trust baseline. The design should confirm this explicitly (it does
not currently).

### 9.4 Concurrent `token new` (non-rotate path)

Two simultaneous `POST .../token` calls without `--renew` and with no existing
token: both pass the "no active token" check, both attempt to insert. The
`UNIQUE (robot_email, name)` constraint will cause one to fail. The server must
handle this as a `409 Conflict`, not a `500 Internal Server Error`. The design
does not specify this error mapping.

---

## Confidence Score

| Item                                                                | Points   | Description                                                                                                                |
| ------------------------------------------------------------------- | -------- | -------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                      | 100      |                                                                                                                            |
| D7: direct caps bypass scope AND invariant                          | -20      | `require_scopes_all` has no path for `robot_direct_caps`; flat union reopens confused-deputy vulnerability from design 003 |
| D7: robot deletion blocked by FK constraint                         | -20      | `builds.user_email` has no `ON DELETE` clause; permanent deletion unimplementable                                          |
| D1: LRU cache integration unspecified                               | -20      | `CachedApiKey` vs `AuthUser` type mismatch; no shared/separate/extended cache decision; invalidation path unspecified      |
| D1: duration parsing unimplementable                                | -20      | `humantime` does not support `d`/`mo`/`y`; `chrono` has no string parser; no crate in workspace handles this grammar       |
| D8: coexistence table omits `PUT .../default-channel`               | -5       | Endpoint accepts robot synthetic email; action on robot not specified                                                      |
| D8: coexistence table omits `POST /api/auth/api-keys`               | -5       | Admin could create API key for robot; design silent on this                                                                |
| D8: `GET /api/auth/whoami` for robot callers unspecified            | -5       | Response shape and `is_robot` field inclusion not defined                                                                  |
| D8: build response `user_email` format for robots unspecified       | -5       | API consumers receive synthetic email; design intent not stated                                                            |
| D10: `last_admin_guard` not updated for `is_robot`                  | -5       | Counts robot wildcard holders; forbidden-cap check is first line of defense but guard itself is unhardened                 |
| D11: `--yes-i-really-mean-it` described as "established" but absent | -5       | Pattern does not exist in codebase; misleads implementation plan                                                           |
| **Total deductions**                                                | **-110** | Score floors at 0                                                                                                          |
| **Final score**                                                     | **0**    |                                                                                                                            |

---

## Findings Ordered by Severity

### Blocking — must resolve in v2 before implementation plan

**B1 (D7): Direct caps break the single-assignment AND scope invariant.**
`require_scopes_all` in `extractors.rs` operates on `AssignmentWithScopes`
records produced by `get_user_assignments_with_scopes`. This query joins
`user_roles → role_caps → role_scopes`. Direct caps in `robot_direct_caps`
produce no such record. There is no specification for how scope enforcement
applies to direct caps. The simplest and safest resolution is to eliminate
`robot_direct_caps` entirely and require all robot permissions to flow through
role assignments.

**B2 (D7): Robot deletion blocked by `builds.user_email` FK.**
`builds.user_email REFERENCES users(email)` with no `ON DELETE` clause means any
robot with build history cannot be deleted without a constraint violation.
Recommended resolution: change deletion semantics to "deactivate only" and
document that permanent deletion is unsupported for robots with build history.
This is consistent with the existing human-user behaviour.

**B3 (D1): LRU cache integration unspecified.** `ApiKeyCache` stores
`CachedApiKey`; the design claims robot token cache stores `AuthUser`. These
types are incompatible. `AppState` has no robot token cache field. The design
must specify the cache type, field name, capacity, and invalidation methods
before the implementation plan can be written.

**B4 (D1): Duration string parsing unimplementable as described.** The
`humantime` crate does not parse `d`, `mo`, or `y` units. `chrono` has no
duration-string parser. The design must specify a custom parser or an
alternative input format.

### Important — resolve in v2

**I1 (D8): Coexistence table incomplete.** Three endpoints are missing:
`PUT /api/admin/users/{email}/default- channel`, `POST /api/auth/api-keys`, and
`GET /api/auth/whoami`. For each, the design must specify whether the endpoint
rejects robot emails, processes them, or is silent.

**I2 (D8): Build response `user_email` for robot callers.** The format and
semantics of `user_email` in build responses when the submitter is a robot must
be specified. API consumers cannot reliably distinguish robot-submitted builds
from human-submitted builds without this.

**I3 (D10): `last_admin_guard` should filter `is_robot = 0`.** Defense-in-depth
hardening. The forbidden-cap check prevents robots from holding `*`, but
`count_active_wildcard_holders` should independently exclude robots.

### Suggestions — address in implementation or v2

**S1 (D11):** Add explicit
`CREATE INDEX idx_robot_tokens_email ON robot_tokens(robot_email)` to the
migration.

**S2 (D11):** Specify `GET /api/robots/{name}/token` endpoint to expose token
metadata (expiry, creation date) without revealing the secret.

**S3 (D11):** Specify the `409 Conflict` mapping for concurrent `token new`
(non-rotate path) hitting the `UNIQUE (robot_email, name)` constraint.

**S4 (D11):** Add cache invalidation step to the robot deletion sequence to
close the window between DB commit and cache expiry.

**S5 (D11):** Confirm zero-cap behaviour explicitly: a robot with no roles and
no direct caps has an empty effective cap set and all requests are rejected.
