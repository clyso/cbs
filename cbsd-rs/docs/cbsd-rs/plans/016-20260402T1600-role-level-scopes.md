# 016 — Role-Level Scopes: Implementation Plan

**Design:**
`docs/cbsd-rs/design/016-20260402T1200-role-level-scopes.md`
(v2-reviewed, approved with findings addressed)

## Implementation Notes

From v2 design review F1-F5:

- **F1 (High):** `ces-devel/*` does NOT match raw channel
  name `ces-devel`. The `periodic.rs` channel scope check
  is broken for non-global patterns. Fix in commit 2
  by removing the redundant raw-channel check — enforcement
  point 4 (`resolve_and_rewrite`) already validates
  `channel/type` scopes at both submission and trigger time.
- **F2 (Medium):** `user_can_view_channel` uses hand-rolled
  inverted matching. Consolidated onto a shared
  `scope_covers_channel` helper in commit 2.
- **F3 (Low):** Three `scopes.is_empty()` sites need audit
  when builder role gains explicit `*` patterns. Audited
  in commit 3 (server role-level scopes).
- **F4 (Low):** Two duplicate `scope_pattern_matches`
  functions. Consolidated into shared `scopes` module in
  commit 2.
- **F5 (Informational):** No API-layer `scope_type`
  validation. Added to role create/update in commit 3.

## Commit Breakdown

4 commits, ordered by dependency.

---

### Commit 1: `cbsd-rs/docs: add role-level scopes design, reviews, and plan`

**Documentation only**

Design document (with v2 review changes applied), both
review documents (v1 and v2), and this plan.

---

### Commit 2: `cbsd-rs/server: consolidate scope matching and fix channel scope bugs`

**~300 authored lines**

Prerequisite cleanup: unify duplicated pattern-matching
logic into a shared module, fix the broken periodic.rs
channel scope check, and refactor visibility matching.
These are pre-existing bugs that become more visible once
scopes are defined centrally on roles.

**What works after this commit:** Periodic task creation
with non-global channel scope patterns (e.g.,
`ces-devel/*`) no longer falsely rejects authorized
users. All four enforcement points use consistent
matching logic.

**Files:**

| File | Change |
|------|--------|
| `cbsd-server/src/scopes.rs` | New module: `scope_pattern_matches(pattern, value)` — single canonical copy; `scope_covers_channel(pattern, channel_name)` — returns true if a channel scope pattern would grant access to any type under the given channel (for visibility checks). Unit tests for both functions covering exact match, wildcard suffix, global `*`, and edge cases. |
| `cbsd-server/src/main.rs` | Register `mod scopes` |
| `cbsd-server/src/auth/extractors.rs` | Remove local `scope_pattern_matches`; import from `crate::scopes` |
| `cbsd-server/src/channels/mod.rs` | Remove local `scope_pattern_matches` and its tests; import from `crate::scopes`. `check_channel_scope` calls `scopes::scope_pattern_matches` directly (no logic change — only the import path changes) |
| `cbsd-server/src/routes/channels.rs` | Refactor `user_can_view_channel`: replace hand-rolled `starts_with` / exact-match logic with `scopes::scope_covers_channel` |
| `cbsd-server/src/routes/periodic.rs` | Fix `validate_descriptor_scopes`: remove the raw channel scope check (`ScopeType::Channel` push at line 590). Repository scope checks remain. The channel scope is already validated downstream by enforcement point 4 (`resolve_and_rewrite` → `check_channel_scope`) at both build-submission and periodic-trigger time, using the correct `channel/type` composite format. |

**Key details:**

- `scope_pattern_matches` semantics are unchanged:
  `*` matches everything, `prefix/*` strips `*` and
  checks `starts_with`, exact otherwise.
- `scope_covers_channel(pattern, channel_name)` returns
  true when `pattern == "*"`, or pattern starts with
  `"{channel_name}/"`, or pattern equals
  `"{channel_name}/*"`. This preserves the existing
  visibility semantics (a user with `ces-devel/dev`
  can see the `ces-devel` channel) while extracting
  the logic into a named, testable helper.
- The `periodic.rs` fix is a deletion, not a rewrite.
  The function continues to check `Repository` scopes
  for component repo overrides — only the broken
  `Channel` scope check is removed.

**Validation:**

```bash
cargo fmt --all
cargo clippy --workspace
SQLX_OFFLINE=true cargo check --workspace
cargo test --workspace
```

---

### Commit 3: `cbsd-rs/server: move scopes from user assignments to role definitions`

**~500-650 authored lines**

The core feature. Schema, DB layer, route handlers, and
seed all change in one commit because the schema rewrite
(`user_role_scopes` → `role_scopes`) invalidates every
query and function that references the old table.

**What works after this commit:** Admins can define scopes
on roles via the API. User-role assignments no longer
carry scopes. All enforcement points read scopes from
the role definition. The seeded `builder` role has
`channel=*` and `repository=*` patterns.

**Files:**

| File | Change |
|------|--------|
| `migrations/001_initial_schema.sql` | Remove `user_role_scopes` table; add `role_scopes` table with `(role_name, scope_type, pattern)` and CHECK constraint |

**DB layer — `cbsd-server/src/db/roles.rs`:**

| Function | Change |
|----------|--------|
| `set_role_scopes(pool, role_name, scopes)` | New: delete-and-reinsert scopes for a role in a transaction |
| `get_role_scopes(pool, role_name)` | New: returns `Vec<ScopeEntry>` for a role |
| `set_user_roles(pool, email, roles)` | Simplify: accept `&[&str]` (role names) instead of `&[RoleAssignment]`; remove all scope insertion logic |
| `get_user_roles(pool, email)` | Simplify: return `Vec<UserRoleWithScopes>` where scopes are read from `role_scopes` (join through `user_roles.role_name`) instead of `user_role_scopes` |
| `get_user_assignments_with_scopes(pool, email)` | Update query: join `role_scopes` on `role_name` instead of `user_role_scopes` on `(user_email, role_name)`. The `user_email` parameter is still needed to filter `user_roles`; scopes come from the role, not the assignment |
| `add_user_role(pool, email, role)` | Simplify: no scope insertion (scopes live on the role) |
| `RoleAssignment` struct | Delete (no longer used) |
| `UserRoleWithScopes` | Rename `scopes` source in doc comment; struct shape unchanged |
| `AssignmentWithScopes` | Unchanged (scopes now come from role) |
| `ScopeEntry` | Unchanged |

**Route handlers —
`cbsd-server/src/routes/permissions.rs`:**

| Area | Change |
|------|--------|
| `CreateRoleBody` | Add `scopes: Vec<ScopeBody>` with `#[serde(default)]` |
| `RoleResponse` | Add `scopes: Vec<ScopeBody>` |
| `RoleAssignmentBody` | Delete |
| `ReplaceUserRolesBody` | Change `roles` field from `Vec<RoleAssignmentBody>` to `Vec<String>` (flat role name list) |
| `AddUserRoleBody` | Remove `scopes` field |
| `create_role` handler | After inserting role + caps, call `db::roles::set_role_scopes`; move scope-dependent validation here (if role has `builds:create`, require at least one scope unless role has `*`) |
| `get_role` handler | Fetch scopes via `get_role_scopes`, include in response |
| `update_role_caps` handler | Accept scopes in body, call `set_role_scopes` in the same transaction; rename to `update_role` |
| `replace_user_roles` handler | Accept `Vec<String>`, pass `&[&str]` to `set_user_roles`; remove scope validation logic |
| `add_user_role` handler | Remove scope insertion; remove scope validation |
| `validate_scopes` | Still called, now from role create/update instead of user assignment |
| New: `validate_scope_type` | Validate `scope_type` is one of `channel`, `registry`, `repository` — return 400 for invalid types instead of letting the DB CHECK constraint produce a 500 |

**Seed — `cbsd-server/src/db/seed.rs`:**

| Role | Change |
|------|--------|
| `admin` | No scopes (wildcard cap, global) — unchanged |
| `builder` | Add `channel=*` and `repository=*` via `set_role_scopes` after role creation |
| `viewer` | No scopes (no scope-dependent caps) — unchanged |

**`scopes.is_empty()` audit:** All three sites
(`require_scopes_all`, `check_channel_scope`,
`user_can_view_channel`) produce correct results
because the builder role's `*` patterns pass through
the pattern-matching path:

1. `require_scopes_all`: `scope_pattern_matches("*", v)`
   → prefix `""` → `v.starts_with("")` → true
2. `check_channel_scope`: same path → true
3. `user_can_view_channel`: `scope_covers_channel("*",
   name)` → `pattern == "*"` → true

No code changes needed at these sites — the audit
confirms correctness with explicit patterns.

**Validation:**

```bash
cargo fmt --all
cargo clippy --workspace
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx database create
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx migrate run
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx prepare --workspace
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

Include `.sqlx/` changes in this commit.

---

### Commit 4: `cbc: move scope management from user commands to role commands`

**~300 authored lines**

The cbc CLI mirrors the API change. Scopes move from
`admin users roles {set,add}` flags to
`admin roles {create,update}` flags.

**What works after this commit:** Admins use
`cbc admin roles create --scope "channel=ces-devel/*"`
to define scopes on roles. User assignment commands
(`cbc admin users roles add --role NAME`) are simpler —
no `--scope` flag, no inline `NAME:TYPE=PAT` syntax.
`cbc admin roles get` displays scopes alongside
capabilities.

**Files:**

**`cbc/src/admin/roles.rs` — add scope support:**

| Area | Change |
|------|--------|
| `ScopeItem` struct | New: `{ scope_type, pattern }` with `serde(rename = "type")` — same shape as the server's `ScopeBody`. Move `parse_scope()` from `users.rs` alongside it. |
| `CreateArgs` | Add `#[arg(long)] scope: Vec<String>` — repeatable `TYPE=PATTERN` |
| `UpdateArgs` | Add `#[arg(long)] scope: Vec<String>` — repeatable `TYPE=PATTERN` |
| `CreateRoleBody` | Add `scopes: Vec<ScopeItem>` with `#[serde(skip_serializing_if = "Vec::is_empty")]` |
| `UpdateRoleBody` | Add `scopes: Vec<ScopeItem>` |
| `RoleDetail` | Add `scopes: Vec<ScopeItem>` with `#[serde(default)]` for backward compatibility with older servers |
| `cmd_create` | Parse `--scope` args via `parse_scope`, include in body |
| `cmd_update` | Parse `--scope` args via `parse_scope`, include in body |
| `cmd_get` | Display scopes after capabilities: `"    scopes: channel = ces-devel/*"` |

**`cbc/src/admin/users.rs` — remove scope support:**

| Area | Change |
|------|--------|
| `RolesSetArgs` | Change `role: Vec<String>` doc comment to "Role name"; remove `NAME:TYPE=PAT` syntax documentation |
| `RolesAddArgs` | Remove `scope: Vec<String>` field entirely |
| `RoleAssignment` struct | Delete |
| `ReplaceUserRolesBody` | Change `roles` field from `Vec<RoleAssignment>` to `Vec<String>` |
| `AddUserRoleBody` | Remove `scopes` field |
| `parse_role_spec()` | Delete |
| `parse_scope()` | Delete (moved to `roles.rs`) |
| `cmd_roles_set` | Simplify: `args.role` is already `Vec<String>` — pass directly as `ReplaceUserRolesBody { roles: args.role }` |
| `cmd_roles_add` | Simplify: `AddUserRoleBody { role: args.role }` — no scope parsing |
| `ScopeItem` | Keep for response deserialization (used in `UserRoleItem`) — or import from `roles.rs` if made `pub(crate)` |

**Key details:**

- `RoleDetail` uses `#[serde(default)]` on the `scopes`
  field so the client works against servers that haven't
  been updated yet. The field deserializes as an empty
  vec when absent.
- `ReplaceUserRolesBody` serializes as
  `{ "roles": ["name1", "name2"] }` — the server
  expects a flat string list after commit 3.
- The `admin users get` display continues to show scopes
  under each role. The `UserRoleItem { role, scopes }`
  response struct is unchanged — scopes now come from
  the role definition rather than the assignment, but
  the JSON shape is identical.

**Validation:**

```bash
cargo fmt --all
cargo clippy --workspace
cargo build --workspace
cargo test --workspace
```

---

## Dependency Graph

```
Commit 1 (docs)
    ↓
Commit 2 (scope matching + bug fixes)
    ↓
Commit 3 (server role-level scopes)
    ↓
Commit 4 (cbc client)
```

Commit 2 is a prerequisite for commit 3: it fixes
the matching functions that the `scopes.is_empty()`
audit in commit 3 depends on. Commit 4 depends on
commit 3 (the server API shape must be finalized
before the client adapts).

## Sizing Notes

Commit 3 (~500-650 lines) is the largest. The schema
change, DB layer, route handlers, and seed are tightly
coupled — the schema rewrite invalidates all queries
that reference `user_role_scopes`, so everything that
touches that table must change in the same commit.
If it exceeds 800 lines during implementation, the
only viable split is to separate `routes/permissions.rs`
changes into "role endpoints gain scopes" and "user
endpoints lose scopes" — but only if each half compiles
independently against the new DB function signatures.

Commit 2 (~300 lines) is small but meaningful: it
delivers a concrete bug fix (periodic.rs) and reduces
code duplication. It is a valid standalone commit.

Commit 4 (~300 lines) is net-negative in lines (more
deletions than additions) but delivers a complete
client-side feature increment.

## Progress

| # | Commit | Status |
|---|--------|--------|
| 1 | docs | Pending |
| 2 | scope matching + bug fixes | Pending |
| 3 | server role-level scopes | Pending |
| 4 | cbc client | Pending |
