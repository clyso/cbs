# Prepopulate Users (Pre-Provisioning)

| Field    | Value                |
| -------- | -------------------- |
| Design   | 020                  |
| Date     | 2026-06-23           |
| Status   | Draft v6             |
| Packages | `cbsd-server`, `cbc` |

## Revision History

- **v6 (2026-06-23)** — addresses design-review v3
  (`020-20260623T2128-design-prepopulate-users-v3.md`, "approve with
  conditions"). Corrects the robot-name normalization mechanism: makes
  `name_to_synthetic_email` the structural choke point (lowercase inside it, so
  all six robot handlers are covered — not just the create path), fixes the
  ordering anchor to "lowercase **before `validate_robot_name`**" on create (the
  v5 "before `name_to_synthetic_email`" anchor would have let the tightened
  validator reject `CI` first), and retracts the false claim that the validator
  backstops the lookup handlers. No model change.
- **v5 (2026-06-23)** — design decision (operator): there is no need for
  uppercase robot names; **all identities are lowercase internally**, which also
  removes case-variant duplicate risk. Replaces the v4 robot carve-out with a
  uniform rule — robot names are lowercased at ingress exactly like human
  emails, so every `users.email` (human and robot) is lowercase and boundary #4
  needs no special-casing. Adds the robot endpoints (`POST /api/admin/robots`
  and `/api/admin/robots/{name}` lookups) to the normalization boundaries and
  tightens `validate_robot_name` to lowercase-only.
- **v4 (2026-06-23)** — addresses design-review v2
  (`020-20260623T1805-design-prepopulate-users-v2.md`). Fixes a regression the
  v3 boundary-#4 wording introduced: blindly lowercasing the shared
  `/api/admin/entity/{email}/…` path param would make uppercase-named robots
  (`robot+CI@robots`) unreachable, so a **robot carve-out** is added —
  case-insensitivity is a human-identity rule only; robot synthetic emails pass
  through verbatim. Adds a one-line `add_entity_role` 404 hardening; specifies
  the `seed_admin` normalization mechanism (config post-load/validate step, not
  serde) and the explicit `params.dev_email` normalization point; drops the
  vestigial single-variant `ProvisionOutcome` enum in favour of returning the
  created `UserRecord`; and adds cbc pending-display and robot-passthrough test
  cases.
- **v3 (2026-06-23)** — addresses design-review v1
  (`020-20260623T1736-design-prepopulate-users-v1.md`). The email-normalization
  story is corrected from "two ingress points" to **every boundary where an
  externally-supplied email enters**: adds normalization of the `seed_admin`
  config value at config load (else a mixed-case `seed_admin` forks a zero-cap
  admin at login — review C1) and of operator-supplied emails on the existing
  admin endpoints (path `{email}` and `revoke-all-tokens` body — review C2; note
  the real failure mode is a loud FK error / zero-row update under
  `foreign_keys = ON`, not a silent success). Clarifies that the domain-allow
  check is **extracted** from `validate_user_info` (new work, not pre-existing
  reuse — review S1); documents the deactivated-human re-provision path (review
  S2); fixes validation-after-normalization ordering (review S3); and adds the
  `cargo sqlx prepare … -- --all-targets` step plus the cbc
  `#[serde(default)] first_login_at` forward-compat field (review S4).
- **v2 (2026-06-23)** — incorporates the confirmed deployment state: production
  users exist, but **all with lowercase emails**. Email-ingress normalization is
  therefore backward-compatible (a no-op on existing rows), so the cross-table
  lowercase migration is moot. `first_login_at` is now **backfilled from
  `created_at`** for existing human rows (login-created rows have `created_at` =
  true first-login time), rather than left NULL, so production users are not
  mislabelled as pending. Greenfield open question resolved.
- **v1 (2026-06-23)** — initial draft.

---

## Overview

Today a `cbsd-rs` user only comes into existence the first time they log in via
Google OAuth. The OAuth callback calls `db::users::create_or_update_user`, which
upserts a `users` row but assigns **no roles**. A brand-new user therefore lands
with zero capabilities and can do nothing until an administrator separately
assigns them a role — a second, manual round-trip that has to happen _after_ the
user shows up.

This design adds **pre-provisioning**: an administrator can create a human user
with zero or more roles _before_ that user has ever logged in. When the user
later authenticates with Google, their account and roles are already in place
and they are productive immediately.

### Goals

- An admin can pre-create a human user with zero or more roles before the user
  has ever logged in.
- Case-insensitive email identity, so a provisioned address always matches the
  address Google returns at login.
- A provisioned-but-never-logged-in user is distinguishable from one who has
  logged in.
- Reuse the existing admin surface and the robot-account creation pattern; no
  new subsystem.

### Non-Goals

- Bulk import (CSV / multi-user request bodies). The single-create endpoint is a
  sufficient primitive; a bulk loop is a trivial follow-up.
- Config-file seeding of arbitrary users. The one-off `seed_admin` bootstrap is
  unchanged.
- Normalizing **pre-existing** mixed-case emails already in the database, and
  Gmail-style dot/plus canonicalization. See
  [Migration & Compatibility](#migration--compatibility).
- Changing the robot lifecycle. Robot creation is touched only to **lowercase
  the robot name** (so robot identities obey the same lowercase invariant);
  create/revive/tombstone/token semantics are otherwise unchanged.

## Background — current identity & role model

The data model already supports pre-provisioning; only an explicit create path
is missing.

- **Users are keyed by email.** `users.email` is the PRIMARY KEY. No Google
  `sub` is stored — identity matching is purely by email string.
- **Roles live in a separate table.** `user_roles(user_email, role_name)` has a
  foreign key to `users(email)` (with `ON DELETE CASCADE`;
  `PRAGMA foreign_keys = ON` is set per connection). A role cannot be assigned
  to a non-existent user.
- **The login upsert preserves roles.** `create_or_update_user` runs
  `INSERT … ON CONFLICT(email) DO UPDATE SET name = excluded.name, …`. It only
  refreshes the display name; rows in `user_roles` are untouched. So inserting a
  user row plus role rows now, and letting the user log in later, leaves the
  pre-assigned roles fully in effect.
- **No human-create endpoint exists.** `routes/admin.rs` can list, activate,
  deactivate, and get/set/add/remove roles for _existing_ entities, but cannot
  create a human user. Humans are created only by OAuth login or the one-off
  `seed_admin` bootstrap in `db::seed::run_first_startup_seed`.
- **Robots already do this.** Robot accounts (design 017) are admin-created
  identities in the same `users` table, with roles assigned at creation, via
  `db::robots::create_or_revive` under a `BEGIN IMMEDIATE` transaction. This
  feature is that pattern applied to human users.
- **Email matching is case-sensitive end-to-end.** The OAuth callback passes
  Google's `email` verbatim to `create_or_update_user`, `paseto::token_create`,
  and `db::tokens::insert_token`. Nothing lowercases it.

## Design

### Identity & email normalization

Email matching must be case-insensitive, otherwise provisioning is a footgun:
provisioning `Alice@example.com` while Google returns `alice@example.com` would
miss the `ON CONFLICT(email)` match, create a second row, orphan the assigned
roles, and the user would log in with **zero** roles — silently defeating the
feature.

Introduce a single normalizer:

```rust
/// Canonical form for an email used as a user identity key.
fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}
```

**The invariant:** every `users.email` is stored lowercase — **human emails and
robot synthetic emails alike** — and every place that turns an externally-
supplied email (or robot name) into a match key normalizes first. There is no
case-sensitive identity anywhere; this also makes case-variant duplicates (`CI`
vs `ci`) impossible. Apply `normalize_email` at **all** such boundaries:

1. **OAuth callback** (`routes/auth.rs`) — normalize the resolved email (the
   Google userinfo `email`, and `params.dev_email` in the dev-mode short-circuit
   _before_ the `GoogleUserInfo` is constructed) ahead of `validate_user_info`,
   `create_or_update_user`, `paseto::token_create`, and `insert_token`.
2. **`seed_admin` config value** — `SeedConfig` derives a plain `serde`
   deserializer with no post-deserialize hook, so normalize `seed_admin`
   explicitly in the config post-load/validate step (alongside the existing
   `oauth.allowed_domains` validation in `config.rs`), not via serde alone. Then
   the bootstrap INSERT in `db::seed::run_first_startup_seed` and the dev-mode
   login (which feeds `seed_admin` back through the callback as `dev_email`)
   agree. Without this, a mixed-case `seed_admin` forks a second `users` row at
   first login — a zero-cap admin that can do nothing.
3. **The provisioning request handler** (new).
4. **Existing admin endpoints that take an operator-supplied email** — the
   `/api/admin/entity/{email}/…` path parameter (roles get/set/add/remove,
   activate/deactivate, default-channel) and the `revoke-all-tokens` request
   body. These currently match verbatim; normalize the operator's input
   **uniformly** (human and robot synthetic emails alike — no special-casing) so
   they honor the case-insensitive identity. (This is the same workflow the
   design recommends for adjusting a provisioned user's roles, so it must be
   consistent.)
5. **Robot endpoints** (`routes/robots.rs`) — `POST /api/admin/robots` (create)
   and every `/api/admin/robots/{name}` handler (get, token rotate, token
   revoke, delete, set-description). The **structural choke point** is
   `name_to_synthetic_email` (`db/robots.rs`): lowercase the name **inside**
   that function, so every synthetic email is lowercase no matter which handler
   builds it — including handlers added later. On the **create** path
   additionally lowercase the name **before `validate_robot_name`** (which runs
   ahead of `name_to_synthetic_email`) and before building the `robot:`-prefixed
   display name, so validation and the stored display name both see the
   canonical form.

**Robot names are lowercase, uniformly.** Because there is no need for uppercase
robot names, the v4 carve-out is dropped in favour of the simpler invariant: a
robot `CI` is stored as `robot+ci@robots` and is reachable through every
endpoint regardless of the case an operator types. The backstop is lowercasing
**inside `name_to_synthetic_email`** — it is the single function all six robot
handlers funnel through to derive the email, so none can construct a mixed-case
identity. (`validate_robot_name` is called **only** on the create path, so it is
_not_ a backstop for the five lookup handlers — the choke point is.) On create,
the name is lowercased before `validate_robot_name`, so tightening that
validator to `[a-z0-9_.-]` is a consistent, self-documenting check that never
wrongly rejects a real request. A side benefit: two robots differing only in
case can no longer coexist — a case-variant `create` collapses to the same
synthetic email and is caught as `AlreadyActive` → 409. (Note: with
`name_to_synthetic_email` lowercasing, the `name_synthetic_email_roundtrip` unit
test must change — uppercase inputs now map to lowercase names.)

**Downstream token-derived lookups need no change.** The `AuthUser` extractor
and `builds.user_email` attribution use the email embedded in the
already-normalized token, so they are canonical by construction.

**Failure mode if a boundary is missed.** A mismatched-case email never silently
"succeeds": under the mandatory `PRAGMA foreign_keys = ON`, an FK-dependent
write such as `add_user_role` raises a foreign-key error (surfaced as a 5xx),
and a match-based `UPDATE`/`DELETE` simply affects zero rows. There is no silent
partial grant — but the operation is confusing, which is why every boundary
normalizes.

**Related one-line hardening.** `add_entity_role` currently treats a missing
user as `unwrap_or(false)` and proceeds to `add_user_role`, surfacing the FK
violation as a 5xx. Since this feature leans on the role endpoints to manage
provisioned users, fix it to return `404` when the (normalized) email has no
user row — matching the `ok_or_else(NOT_FOUND)?` pattern already used by
`deactivate_entity`.

Lowercasing only (not full RFC canonicalization) is deliberate: it is the
pragmatic rule that matches how Google and every real IdP treat addresses,
without the surprises of dot/plus stripping.

### Provisioning model

A new database function mirrors `db::robots::create_or_revive`:

```rust
pub enum ProvisionUserError {
    AlreadyExists,        // a human row already exists → 409
    RobotCollision,       // an is_robot=1 row holds this email → 409
    RobotNamePrefix,      // name starts with "robot:" → 403
    UnknownRole(String),  // a requested role does not exist → 400
    UniqueViolation,      // residual UNIQUE race → 409
    Db(sqlx::Error),      // → 500
}

// Returns the freshly created user record (used to build the 201 response).
// No success-variant enum: unlike robots, humans are never "revived", so the
// only success outcome is a fresh create.
pub async fn provision_user(
    pool: &SqlitePool,
    email: &str,          // already normalized by the caller
    name: &str,
    roles: &[&str],
) -> Result<UserRecord, ProvisionUserError>;
```

Behaviour, under `BEGIN IMMEDIATE`, re-reading the row under the write lock
(same isolation discipline as `create_or_revive`, so concurrent creates
serialize and a loser returns 409 rather than a raw 500):

- **No existing row** → INSERT the user (`is_robot = 0`, `active = 1`,
  `first_login_at = NULL`, `name` = provided-or-placeholder) and INSERT each
  `user_roles` row. Returns the new `UserRecord`.
- **Existing `is_robot = 1` row** → `RobotCollision`.
- **Existing `is_robot = 0` row** (already provisioned, already logged in, _or
  deactivated_) → `AlreadyExists`. The `active` flag is not consulted: a
  deactivated human is **not** revived by re-provisioning (unlike a tombstoned
  robot, which `create_or_revive` reactivates).

**Idempotency decision — reject, do not merge.** Provisioning is a _create_.
Adjusting an existing user's roles is already served by
`PUT/POST /api/admin/entity/{email}/roles`. Rejecting a duplicate keeps the
semantics unambiguous and mirrors the robot `AlreadyActive` behaviour. (If
reviewers prefer merge-on-conflict — add the roles to the existing user — that
is a localized change to the "existing row" branch.)

The remediation for an existing user — including a **deactivated** one — is the
existing admin surface, not re-provisioning:
`PUT /api/admin/entity/{email}/activate` to re-enable, then the role endpoints
to adjust roles. Re-provisioning is deliberately not an "upsert that
reactivates" so that a disabled account is never silently brought back by a
provisioning call.

Validation runs on the **normalized** email and the provided name — the request
handler normalizes the email _first_, then every check below operates on the
canonical value (so the robot-email and domain checks cannot be bypassed by
case):

- Reject a `name` starting with the reserved `robot:` prefix — the same
  SSO-forgery guard `create_or_update_user` already enforces (`RobotNamePrefix`
  → 403).
- Reject robot synthetic emails (`robot+…@robots`).
- Validate every requested role exists, returning `UnknownRole` for a clear
  error rather than an opaque foreign-key failure. An empty role list is allowed
  (provision an account now, assign roles later).
- Validate the email's **domain** against the same `oauth.allowed_domains` /
  `oauth.allow_any_google_account` configuration used at login. This logic is
  currently **inline** in `oauth::validate_user_info` (and gated behind
  `email_verified`); it must be **extracted** into a reusable, verification-
  independent helper (e.g. `oauth::is_email_domain_allowed`) that both the login
  path and provisioning call. This is new work, not pre-existing reuse. It
  prevents provisioning an address that could never log in.

**Default name.** When the caller omits a name, default to the email local-part
(the portion before `@`). Google's real display name overwrites it on first
login.

**Last-admin guard.** Not engaged. Creation only ever _adds_ an entity (and at
most adds an admin); it never removes the last wildcard holder, so the existing
`last_admin_guard` does not apply here.

### Login-state tracking

Add a nullable column to record first login:

- `users.first_login_at INTEGER` — nullable, no default. `NULL` is the sentinel
  for "provisioned, never logged in".

The login upsert in `create_or_update_user` becomes:

```sql
INSERT INTO users (email, name, first_login_at)
VALUES (?, ?, unixepoch())
ON CONFLICT(email) DO UPDATE SET
    name = excluded.name,
    updated_at = unixepoch(),
    first_login_at = COALESCE(users.first_login_at, unixepoch());
```

Cases:

- **Direct first login** (never provisioned) → INSERT stamps `first_login_at` to
  now.
- **Provisioned user's first login** → `COALESCE(NULL, now)` = now.
- **Returning user** → `COALESCE(existing, now)` = existing, preserved.

So `first_login_at IS NULL` means exactly "provisioned but not yet logged in".

Two consequences that callers must respect:

- **Existing rows are backfilled from `created_at`.** Every existing human row
  was created _at its first login_ — the login upsert INSERTs the row on first
  auth and never rewrites `created_at` afterwards — so `created_at` is an
  accurate first-login timestamp for login-created users. The migration
  therefore backfills `first_login_at = created_at` for human rows rather than
  leaving them NULL, which would otherwise mislabel every real production user
  as "pending" until their next login. The one inaccuracy is a `seed_admin` row
  that was bootstrapped but never logged in: it is wrongly "un-pended" (shown as
  having logged in at bootstrap time). This is an accepted edge case — there is
  no DB signal that distinguishes a bootstrapped-only admin from a logged-in
  one, and in practice the production admin has logged in. See
  [Migration & Compatibility](#migration--compatibility).
- **Robots are NULL forever.** `create_robot_in_conn` and `revive_robot_in_conn`
  never set `first_login_at`, so robots always carry NULL. The "pending" marker
  must therefore be gated on `is_robot = 0`; a service account that never logs
  in must not display as "pending".

### Database schema changes

Migration `010_user_first_login.sql`:

```sql
ALTER TABLE users ADD COLUMN first_login_at INTEGER;

-- Backfill: existing human rows were created at their first login (the login
-- upsert INSERTs the row on first auth and never rewrites created_at), so
-- created_at is an accurate first-login time for them. Robots never log in and
-- stay NULL. A never-logged-in seed_admin is marginally un-pended; accepted.
UPDATE users SET first_login_at = created_at WHERE is_robot = 0;
```

### REST API

**Create a user** — `POST /api/admin/entities`, capability `permissions:manage`
(the same capability that guards role assignment).

This is the create verb on the same collection that `GET /api/admin/entities`
already lists. It creates **human users only**; robots have their own lifecycle
endpoint at `POST /api/admin/robots`. (`POST /api/admin/users` is a reasonable
alternative name; noted for reviewers, trivially renameable.)

Request body:

```json
{
  "email": "alice@example.com",
  "name": "Alice",
  "roles": ["builder"]
}
```

- `email` — required. Normalized (lowercased) server-side.
- `name` — optional. Defaults to the email local-part.
- `roles` — optional array of existing role names; may be empty.

Response `201 Created` returns the new entity, including `first_login_at: null`:

```json
{
  "email": "alice@example.com",
  "name": "Alice",
  "active": true,
  "is_robot": false,
  "first_login_at": null,
  "roles": ["builder"]
}
```

Error → status mapping (mirrors the robot create handler):

| Condition                              | Status |
| -------------------------------------- | ------ |
| `AlreadyExists` / `RobotCollision`     | 409    |
| `UniqueViolation` (concurrent race)    | 409    |
| `RobotNamePrefix`                      | 403    |
| `UnknownRole` / bad email / bad domain | 400    |
| `Db`                                   | 500    |

**List / detail** — `GET /api/admin/entities` (and the per-entity roles view)
gain `first_login_at` on the response item (`EntitySummary` plus the
`EntityWithRolesItem` response type), so clients can distinguish pending from
active.

### CLI (`cbc`)

In `cbsd-rs/cbc/src/admin/users.rs`:

- **`cbc admin users create <email> [--name <name>] [--role <name>]…`** —
  `--role` is repeatable (mirroring the robot `create` command). Calls
  `POST /api/admin/entities`. Prints the created user and its assigned roles.
- **`cbc admin users list` / `get`** render login state — the first-login date,
  or a `pending` marker when `first_login_at` is null. The pending marker is
  gated on `is_robot = 0`. `users list`/`get` already query `type=user`, but the
  shared `first_login_at` field is also returned for `type=robot|all`, so any
  renderer must avoid labelling a robot (always NULL) as pending.

## Migration & Compatibility

**Confirmed deployment state:** this `cbsd-rs` instance has production users,
but **all with lowercase emails**. That resolves the two compatibility concerns
raised in v1:

1. **Email lowercasing is backward-compatible.** Because every existing
   `users.email` is already lowercase, `normalize_email` is a no-op on existing
   rows: no duplicate rows, no orphaned roles, no behavioural change for current
   users. A cross-table lowercase data migration is therefore **not needed**.
   (Had any mixed-case rows existed, such a migration would have been required
   and risky, since `users.email` is a PRIMARY KEY referenced by foreign keys
   **without** `ON UPDATE CASCADE` — `user_roles`, `tokens`, `api_keys`,
   `builds.user_email`, …. Going forward, ingress normalization keeps every new
   row lowercase, so the invariant is maintained.) **This covers robots too:**
   robot rows live in `users` with synthetic emails, so the guard query below
   also flags any uppercase robot identity; the same FK caveat applies to
   `robot_tokens.robot_email` if one is ever found and must be lowercased.
2. **`first_login_at` is backfilled from `created_at`** for human rows (see
   [Login-state tracking](#login-state-tracking)), so existing production users
   show their true first-login time rather than mislabelling as pending. The
   only residual inaccuracy is a never-logged-in `seed_admin` row, which is
   accepted.

**Operational guard:** the backward-compatibility of email lowercasing depends
on the "all identities already lowercase" fact. Before shipping, verify it holds
for both humans and robots with one query —
`SELECT email FROM users WHERE email <> lower(email);` must return no rows
(robot rows are in `users`, so this covers `robot+…@robots` identities as well).

## Security Considerations

- Provisioning requires `permissions:manage`, identical to role assignment — no
  new privilege boundary.
- The `robot:` name guard and robot-synthetic-email guard prevent a human
  provision from impersonating a service identity.
- Provisioning events are logged (email + assigned roles + acting admin) for
  audit, consistent with other admin mutations.
- Domain validation at provisioning ensures an admin cannot create an account
  that can never authenticate.
- Normalizing email at **every** ingress boundary (including the `seed_admin`
  config value) prevents identity-forking: a case mismatch can never split one
  person across two `users` rows, which would otherwise strand roles — including
  silently leaving a bootstrap admin with zero capabilities.

## Testing

- **DB unit tests** (`db/users.rs`): provision → login round-trip preserves
  roles and updates the name; `first_login_at` is set on first login and
  preserved thereafter; duplicate provision → `AlreadyExists`; robot email →
  `RobotCollision`; unknown role → `UnknownRole`; `robot:` name →
  `RobotNamePrefix`; case-insensitive matching (provision mixed-case, log in
  lowercase → single row).
- **Route tests**: status-code mapping for each error; `permissions:manage`
  enforcement; an operator-supplied **mixed-case** human email on an entity role
  endpoint resolves to the same lowercased user (regression for review v1 C2);
  `add_entity_role` on a non-existent email returns `404`, not `5xx`.
- **Robot lowercase tests**: creating a robot named `CI` stores
  `robot+ci@robots` (lowercased); a case-variant `create` (`CI` then `ci`)
  collapses to one identity and the second returns `409`; the robot is reachable
  through both the `/api/admin/robots/{name}` and shared `/entity/{email}/…`
  endpoints regardless of the case typed; `validate_robot_name` rejects
  uppercase.
- **Seed test**: a mixed-case `seed_admin` config produces exactly one `users`
  row and the admin retains the wildcard cap after login — no forked zero-cap
  admin (regression for review v1 C1).
- **cbc display test**: a robot row (`first_login_at = NULL`, `is_robot = 1`) is
  **not** rendered as "pending"; a never-logged-in provisioned human
  (`first_login_at = NULL`, `is_robot = 0`) **is**.
- **Manual end-to-end** (dev mode): `cbc admin users create` then `get` shows
  the role and a `pending` state; logging in with `?dev-email=` flips it to a
  first-login date with roles intact.

## Implementation Plan

A companion plan document under `cbsd-rs/docs/cbsd-rs/plans/` (sequence 020)
tracks the commit-level breakdown. Summary:

1. **Case-insensitive identity + first-login time** — migration `010` (add
   `first_login_at` + backfill); `normalize_email`; normalization at every
   boundary (OAuth callback, `seed_admin` at config load, the operator-supplied-
   email admin endpoints, the robot create + `/api/admin/robots/{name}`
   endpoints); `validate_robot_name` tightened to lowercase-only (update its
   tests); the `create_or_update_user` upsert change; `first_login_at` on the
   entity-list response and `cbc` rendering (with
   `#[serde(default)] first_login_at: Option<i64>` on the cbc `UserWithRoles`
   deserialization struct for forward-compat); the one-line `add_entity_role`
   404 hardening. Delivers case-insensitive identity and first-login visibility
   on its own.
2. **Provision users with roles before first login** — extract
   `oauth::is_email_domain_allowed`; `provision_user`; the
   `POST /api/admin/entities` handler; and `cbc admin users create`.

**Implementation notes**

- After either commit touches an `sqlx` query (the new nullable column changes
  the list queries; `provision_user` adds new ones), regenerate the offline
  cache:
  `DATABASE_URL=sqlite:///tmp/cbsd-dev.db cargo sqlx prepare --workspace -- --all-targets`
  (the `-- --all-targets` is required — test modules hold `sqlx::query!`).
- Per-commit pre-checks: `cargo fmt --all`, `cargo clippy --workspace`,
  `cargo check --workspace` (all clean) before staging.

## Alternatives Considered

- **Config-file seeding** (extend `SeedConfig` with a user list, like
  `seed_admin`/`seed_workers`). Rejected as the primary mechanism: bootstrap
  only, awkward to re-run against an existing database, and not how day-to-day
  provisioning happens. The runtime admin API is the chosen path.
- **`COLLATE NOCASE` on `users.email`** instead of lowercasing at ingress.
  Robust and code-change-free at the call sites, but requires rebuilding a
  PRIMARY KEY column referenced by several foreign keys — a heavier migration
  than the feature warrants. Lowercasing at ingress is migration-free.
- **No login-state column** (placeholder name only). Simpler, but provides no
  way to tell a provisioned user from one who has logged in, which operators
  explicitly want.
- **Merge-on-conflict** instead of rejecting duplicates. Deferred; the role
  endpoints already cover modifying an existing user.

## Open Questions

- Endpoint naming: `POST /api/admin/entities` (chosen) vs
  `POST /api/admin/users`.
- Whether the `seed_admin` un-pend edge (a bootstrapped-but-never-logged-in
  admin shown as logged-in after backfill) is worth any extra handling, or is
  acceptable as-is (current stance: acceptable).
