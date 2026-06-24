# 020 — Prepopulate Users (Pre-Provisioning): Implementation Plan

**Design:** `docs/cbsd-rs/design/020-20260623T1724-prepopulate-users.md` (Draft
v6)

**Reviews:**

- `docs/cbsd-rs/reviews/020-20260623T1736-design-prepopulate-users-v1.md`
- `docs/cbsd-rs/reviews/020-20260623T1805-design-prepopulate-users-v2.md`
- `docs/cbsd-rs/reviews/020-20260623T2128-design-prepopulate-users-v3.md`

## Scope

Let an admin pre-create a human user with zero or more roles **before** that
user logs in via Google OAuth, so roles are in effect on first login. The data
model already supports it (email-keyed `users`, roles in a separate FK'd table
that the login upsert leaves untouched); the missing piece is an explicit create
path plus the supporting invariants.

Two supporting changes make the feature correct and observable:

1. **Case-insensitive identity, everywhere.** Every `users.email` — human and
   robot synthetic emails alike — is lowercase, and every boundary that turns an
   externally-supplied email or robot name into a match key normalizes first.
   Without this, a provisioned `Alice@x.com` would never match a login as
   `alice@x.com` and the assigned roles would silently orphan.
2. **First-login tracking.** A nullable `users.first_login_at` distinguishes a
   provisioned-but-never-logged-in user (`NULL`) from one who has logged in.

**Deployment fact:** production users exist but all emails are lowercase, so the
normalization change is backward-compatible (a no-op on existing rows) and needs
no data migration. Verify before shipping commit 2:
`SELECT email FROM users WHERE email <> lower(email);` must return no rows
(covers robots too — their rows live in `users`).

## Commit Breakdown

4 commits, ordered by dependency. Auto-generated `.sqlx/` and `Cargo.lock` do
not count toward the authored-LOC estimates.

| #   | Commit                                                          | ~LOC | Status |
| --- | --------------------------------------------------------------- | ---- | ------ |
| 1   | `cbsd-rs/docs: add user-prepopulation design, reviews, plan`    | docs | Done   |
| 2   | `cbsd-rs/server: lowercase email + robot-name identity`         | ~450 | Done   |
| 3   | `cbsd-rs/server: record first-login time and surface it`        | ~350 | Done   |
| 4   | `cbsd-rs/server: provision users with roles before first login` | ~500 | Done   |

Design-summary item 1 ("case-insensitive identity + first-login time") maps to
plan commits 2 and 3; item 2 ("provision users") maps to commit 4.

---

### Commit 1: `cbsd-rs/docs: add user-prepopulation design, reviews, plan`

**Documentation only.** Tracks the design doc, the three design reviews, and
this plan.

| File                                                         | Change     |
| ------------------------------------------------------------ | ---------- |
| `docs/cbsd-rs/design/020-20260623T1724-prepopulate-users.md` | New (v6)   |
| `docs/cbsd-rs/reviews/020-…-design-prepopulate-users-v1.md`  | New        |
| `docs/cbsd-rs/reviews/020-…-design-prepopulate-users-v2.md`  | New        |
| `docs/cbsd-rs/reviews/020-…-design-prepopulate-users-v3.md`  | New        |
| `docs/cbsd-rs/plans/020-20260623T2118-prepopulate-users.md`  | New (this) |

> The `plans/README.md` phase table lapsed after Phase 13 / design 017 (018 and
> 019 have plan docs but no table rows). Adding only a 020 row would be a
> misleading partial update, and back-filling 018/019 is out of scope, so the
> table is left as-is.

---

### Commit 2: `cbsd-rs/server: lowercase email + robot-name identity` (~450)

Establish the uniform lowercase-identity invariant. Delivers, on its own:
case-insensitive login (no duplicate rows from case variance), case-insensitive
admin operations, and lowercase robot identities (case-variant duplicate robots
become impossible). No schema change.

**`normalize_email(email: &str) -> String`** = `email.trim().to_lowercase()`
(new shared helper; home it where both `routes` and `db`/`auth` can call it,
e.g. `auth/mod.rs` or a small `util`).

Apply normalization at every boundary:

| File                               | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cbsd-server/src/routes/auth.rs`   | Lowercase the resolved email in the OAuth callback — the Google userinfo `email`, and `params.dev_email` in the dev short-circuit before `GoogleUserInfo` is built — ahead of `validate_user_info`, `create_or_update_user`, `paseto::token_create`, `insert_token`.                                                                                                                                                                                                                                                                                  |
| `cbsd-server/src/config.rs`        | Normalize `seed_admin` in the config post-load/validate step (serde has no post-hook), so the bootstrap INSERT and the dev-mode `dev_email` round-trip agree.                                                                                                                                                                                                                                                                                                                                                                                         |
| `cbsd-server/src/routes/admin.rs`  | Normalize the operator-supplied `{email}` path param on the `/api/admin/entity/{email}/…` endpoints (roles get/set/add/remove, activate/deactivate, default-channel) and the `revoke-all-tokens` body — **uniformly** (human + robot synthetic emails). Add the one-line `add_entity_role` 404: a missing user must `ok_or_else(NOT_FOUND)?` rather than `unwrap_or(false)` → FK 5xx.                                                                                                                                                                 |
| `cbsd-server/src/db/robots.rs`     | **Choke point:** lowercase the name **inside `name_to_synthetic_email`**, so every robot synthetic email is lowercase regardless of caller — this structurally covers all six robot handlers (create, get, token rotate, token revoke, delete, `set_robot_description`). Tighten `validate_robot_name` charset to lowercase-only (`[a-z0-9_.-]`). Update unit tests: `name_synthetic_email_roundtrip` (uppercase inputs now map to lowercase names), the `validate_robot_name` `A1B2`/`CI.NIGHTLY` cases, and add a lowercase-collapse `create` test. |
| `cbsd-server/src/routes/robots.rs` | On the **create** path (`POST /api/admin/robots`), lowercase the name **before `validate_robot_name`** (which runs ahead of `name_to_synthetic_email`) and before building the `robot:`-prefixed display name. The five lookup handlers (get, token rotate, token revoke, delete, `set_robot_description` at line 784) need no per-handler change — they are covered by the choke point above, which they already funnel through; verify each does.                                                                                                   |

> **Client note (verified via impact analysis):** `cbc` has its own
> `name_to_synthetic_email` (`cbc/src/admin/robots.rs`), used by the robot
> enable/disable/roles/default-channel commands to call the shared
> `/api/admin/entity/{email}/…` endpoints. Those endpoints are normalized
> server-side by boundary #4 above, so `cbc` is covered transitively — no `cbc`
> change is required for correctness (the server is the authority).

**Tests:** case-insensitive login (mixed-case → single row, roles intact);
mixed-case email on an entity role endpoint resolves to the lowercased user;
`add_entity_role` on a non-existent email → 404; robot `CI` stored as
`robot+ci@robots`; case-variant `create` collapses → 409; robot reachable via
both `/robots/{name}` and `/entity/{email}/…` regardless of typed case.

---

### Commit 3: `cbsd-rs/server: record first-login time and surface it` (~350)

Add the login-state column and make it observable. Delivers first-login
visibility on its own.

| File                                  | Change                                                                                                                                                                                                                                                                   |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `migrations/010_user_first_login.sql` | `ALTER TABLE users ADD COLUMN first_login_at INTEGER;` then `UPDATE users SET first_login_at = created_at WHERE is_robot = 0;` (login-created human rows have `created_at` = true first-login time; robots stay NULL).                                                   |
| `cbsd-server/src/db/users.rs`         | `create_or_update_user` upsert: `INSERT … VALUES (?, ?, unixepoch()) ON CONFLICT DO UPDATE SET name=…, updated_at=unixepoch(), first_login_at = COALESCE(users.first_login_at, unixepoch())`. Add `first_login_at: Option<i64>` to `EntitySummary` and the list queries. |
| `cbsd-server/src/routes/admin.rs`     | Add `first_login_at` to the entity-list / per-entity response item.                                                                                                                                                                                                      |
| `cbc/src/admin/users.rs`              | Render login state in `list`/`get` (first-login date, or `pending` when null), gated on `is_robot = 0`. Add `#[serde(default)] first_login_at: Option<i64>` to the `UserWithRoles` deserialization struct.                                                               |

**Tests:** `first_login_at` set on first login and preserved after; robot row
(NULL, `is_robot=1`) not rendered "pending"; never-logged-in provisioned human
(NULL, `is_robot=0`) rendered "pending".

---

### Commit 4: `cbsd-rs/server: provision users with roles before first login` (~500)

The capability itself. Depends on commits 2 (lowercase identity) and 3
(`first_login_at`).

| File                              | Change                                                                                                                                                                                                                                                                                                                                                                                                        |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cbsd-server/src/auth/oauth.rs`   | Extract the domain-allow logic from `validate_user_info` into a reusable, verification-independent helper (`is_email_domain_allowed`) called by both login and provisioning.                                                                                                                                                                                                                                  |
| `cbsd-server/src/db/users.rs`     | `provision_user(pool, email, name, roles) -> Result<UserRecord, ProvisionUserError>` under `BEGIN IMMEDIATE`, re-read under the write lock: no row → INSERT user + `user_roles`; `is_robot=1` → `RobotCollision`; `is_robot=0` (any `active`) → `AlreadyExists`. Reject `robot:` names and `robot+…@robots` emails; validate each role exists (`UnknownRole`); normalize residual UNIQUE → `UniqueViolation`. |
| `cbsd-server/src/routes/admin.rs` | `POST /api/admin/entities` (cap `permissions:manage`): normalize + domain-validate the email, default name to the local-part, call `provision_user`, map errors (409/403/400/500), return 201 with the entity (incl. `first_login_at: null`). Wire the route.                                                                                                                                                 |
| `cbc/src/admin/users.rs`          | `cbc admin users create <email> [--name <name>] [--role <name>]…` (repeatable `--role`) → `POST /api/admin/entities`; print the created user + roles.                                                                                                                                                                                                                                                         |

**Tests:** provision → login round-trip (roles intact, name updated,
`first_login_at` NULL→set); duplicate → 409; robot email → 409; unknown role →
400; disallowed domain → 400; `robot:` name → 403; `permissions:manage`
enforced.

## Verification

- Per commit: `cargo fmt --all`, `cargo clippy --workspace`,
  `cargo check --workspace`, `cargo test --workspace` — all clean.
- After any commit touching sqlx queries (commit 3's column changes the list
  queries; commit 4 adds queries):
  `DATABASE_URL=sqlite:///tmp/cbsd-dev.db cargo sqlx prepare --workspace -- --all-targets`,
  then `SQLX_OFFLINE=true cargo build --workspace`; commit the `.sqlx/` changes.
- Manual end-to-end (dev mode,
  `podman-compose -f podman-compose.cbsd-rs.yaml up`):
  `cbc admin users create alice@<allowed-domain> --role builder` → 201;
  `cbc admin users get alice@…` shows the role + `pending`; provision mixed-case
  and log in via `?dev-email=` → single row, roles intact, no longer pending;
  duplicate → 409; unknown role → 400; disallowed domain → 400.

## Notes

- Update the progress table above and the README Phase entry after each commit
  lands.
- Greenfield guard: run the `email <> lower(email)` check on the target DB
  before shipping commit 2.
