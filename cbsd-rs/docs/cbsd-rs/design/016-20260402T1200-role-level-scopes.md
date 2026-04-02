# Role-Level Scopes

**Supersedes:** §Scopes and §User-role assignments in
[003 — Auth & Permissions Design][003].

## Problem

Scopes currently live on **user-role assignments** — each time a role
is assigned to a user, the admin must also specify which channel,
registry, and repository patterns that user may access. For a team of
30 builders who all need the same access, the admin must repeat the
same scope set 30 times. There is no way to say "the `prod-builder`
role inherently carries these scopes."

This makes large-scale permission management painful and error-prone:

- Adding a new channel pattern requires updating every affected
  user-role assignment individually.
- Auditing "who can build for ces-prod" requires inspecting every
  user's assignment scopes rather than looking at a role definition.
- Custom roles (e.g., `prod-builder`, `staging-builder`) already
  encode intent in their names and capabilities, but their scopes
  must be re-specified per user.

## Design

Move scopes from the user-role assignment to the **role definition**.
A role now defines both *what* you can do (capabilities) and *where*
you can do it (scopes). Assigning a role to a user grants them all
of the role's capabilities within all of the role's scopes.

### Data model

**Before:**

```
Role  ──has──>  Capabilities
User  ──assigned──>  Role  ──with──>  Scopes (per assignment)
```

**After:**

```
Role  ──has──>  Capabilities
Role  ──has──>  Scopes
User  ──assigned──>  Role
```

The `user_role_scopes` table is **removed**. A new `role_scopes`
table is introduced:

```sql
CREATE TABLE IF NOT EXISTS role_scopes (
    role_name   TEXT NOT NULL
                REFERENCES roles(name) ON DELETE CASCADE,
    scope_type  TEXT NOT NULL
                CHECK (scope_type IN (
                    'channel', 'registry', 'repository'
                )),
    pattern     TEXT NOT NULL,
    UNIQUE (role_name, scope_type, pattern)
);
```

The `user_roles` table remains unchanged — it is a simple
`(user_email, role_name)` join table with no scope fields.

### Scope semantics (unchanged)

The three scope types, glob matching, and AND-within-assignment
semantics from [003][] are preserved:

| Scope type   | Checked against                     |
|--------------|-------------------------------------|
| `channel`    | `channel/type` composite (resolved) |
| `registry`   | hostname from `descriptor.dst_image`|
| `repository` | `descriptor.components[].repo`      |

**Assignment-level AND semantics** remain: all scope checks for a
single operation must be satisfied by the scopes of a **single**
role. The system does not combine scopes across different roles.

**Empty scopes = global access.** A role with no scope entries
grants its capabilities globally. This preserves the behaviour of
`admin` (wildcard cap, no scopes) and `viewer` (no scope-dependent
caps).

### Scope evaluation

Given a user with roles R1, R2, …, and a set of scope checks
S = [(type1, value1), (type2, value2), …]:

```
authorized = any role Ri where:
    Ri.scopes is empty              → pass (global)
    OR for every (type, value) in S:
        exists scope in Ri.scopes where
            scope.scope_type == type
            AND scope_pattern_matches(scope.pattern, value)
```

This is the same algorithm as today — only the data source changes
(from `user_role_scopes` to `role_scopes`).

### Example: before and after

**Before** (per-assignment scopes):

| user         | role      | scope (on assignment)            |
|--------------|-----------|----------------------------------|
| alice@clyso  | builder   | channel=ces-devel/*              |
| bob@clyso    | builder   | channel=ces-prod/*               |
| carol@clyso  | builder   | channel=ces-devel/*              |
| dave@clyso   | builder   | channel=ces-devel/*              |

Problem: three users share the exact same scope set, specified
three times.

**After** (role-level scopes):

| role             | caps                         | scopes             |
|------------------|------------------------------|---------------------|
| devel-builder    | builds:create, ...           | channel=ces-devel/* |
| prod-builder     | builds:create, ...           | channel=ces-prod/*  |

| user         | role           |
|--------------|----------------|
| alice@clyso  | devel-builder  |
| bob@clyso    | prod-builder   |
| carol@clyso  | devel-builder  |
| dave@clyso   | devel-builder  |

Adding a new scope pattern is now a single role update. New users
get the right scopes by role assignment alone.

### Trade-off: no per-user scope overrides

Per-user scope customization within a single role is intentionally
removed. If two users need the same capabilities but different
scopes, create two roles (e.g., `devel-builder` and
`prod-builder`). This is the expected mechanism — the scaling
benefit of role-level scopes comes from collapsing N identical
per-user scope sets into one role definition.

## Enforcement Points

Four places in the codebase gate access using scopes. All four
go through `AuthUser::require_scopes_all()` or
`db::roles::get_user_assignments_with_scopes()`. The enforcement
logic is unchanged — only the underlying query changes.

### 1. Build submission

**File:** `cbsd-server/src/routes/builds.rs` — `submit_build`

Checks `Repository` scopes for component repo overrides. Calls
`user.require_scopes_all(&pool, &scope_refs)`.

**Change needed:** None. The `require_scopes_all` method is
transparent to the scope source.

### 2. Periodic build validation

**File:** `cbsd-server/src/routes/periodic.rs` —
`validate_descriptor_scopes`

Checks `Channel` and `Repository` scopes from the periodic task's
stored descriptor. Calls `user.require_scopes_all(...)`.

**Change needed:** Fix the channel scope check to use the
`channel/type` composite format instead of the raw channel name.
See §Note: channel scope value format inconsistency below.

### 3. Channel visibility

**File:** `cbsd-server/src/routes/channels.rs` —
`user_can_view_channel()`

Calls `get_user_assignments_with_scopes()` directly and checks
whether any assignment's `Channel` scopes reference the channel
name.

**Matching asymmetry:** Unlike the other three enforcement points,
this function does **not** use `scope_pattern_matches()`. It uses
inverted matching — checking whether the **pattern** references
the channel name (e.g., `pattern.starts_with("ces-devel/")` or
`pattern == "ces-devel/*"`), rather than checking a value against
the pattern. This is intentional: a user with scope
`ces-devel/dev` should see the `ces-devel` channel even though
`scope_pattern_matches("ces-devel/dev", "ces-devel")` would
return false.

**Change needed:** Refactor to use `scope_pattern_matches` with a
synthesized `channel_name/*` value to check visibility, aligning
the matching logic with the other enforcement points. See
§Implementation Scope.

### 4. Channel/type scope resolution

**File:** `cbsd-server/src/channels/mod.rs` —
`check_channel_scope()`, called from `resolve_and_rewrite()`

Calls `get_user_assignments_with_scopes()` directly and checks
whether any assignment's `Channel` scopes match the resolved
`channel/type` composite value (e.g., `"ces-devel/dev"`). Reached
from two paths:

- `routes/builds.rs` — build submission (after the Repository
  scope check, so a single request hits two independent scope
  gates).
- `scheduler/trigger.rs` — periodic build trigger execution.

**Change needed:** None. Same reasoning as channel visibility.

### Note: channel scope value format inconsistency (fix required)

Enforcement points 2 and 4 both check `Channel` scopes, but
against different value formats:

- **Point 2** (`validate_descriptor_scopes`) checks the **raw**
  channel name from the descriptor (e.g., `"ces-devel"`).
- **Point 4** (`check_channel_scope`) checks the **resolved**
  `channel/type` composite (e.g., `"ces-devel/dev"`).

A pattern like `ces-devel/*` does **not** match the raw channel
name `"ces-devel"`. The `scope_pattern_matches` function strips
the trailing `*` to produce prefix `"ces-devel/"`, and
`"ces-devel".starts_with("ces-devel/")` is false. Only the
global wildcard `*` matches both formats.

This means enforcement point 2 is **broken** for any non-global
channel scope pattern: a user with `ces-devel/*` would fail the
periodic task creation check even though they are authorized for
all types under that channel.

**Resolution:** Harmonize enforcement point 2 as part of this
change. `validate_descriptor_scopes` must construct the
`channel/type` composite (e.g., `"ces-devel/dev"`) the same way
enforcement point 4 does, rather than passing the raw channel
name. If the descriptor does not contain a type at validation
time, the check should be skipped (the type is resolved later
in `resolve_and_rewrite`, which already performs its own channel
scope check via enforcement point 4).

### Registry scope type

`ScopeType::Registry` is defined in the enum but **never checked**
at any enforcement point today. It remains available for future use
but has no active gates. No change needed.

## API Changes

### Role endpoints (scopes added)

**`POST /api/permissions/roles`** — create role with scopes:

```json
{
  "name": "devel-builder",
  "description": "Builder for ces-devel channels",
  "caps": ["builds:create", "builds:revoke:own",
           "builds:list:own", "builds:list:any",
           "apikeys:create:own"],
  "scopes": [
    { "type": "channel", "pattern": "ces-devel/*" },
    { "type": "registry",
      "pattern": "harbor.clyso.com/ces-devel/*" }
  ]
}
```

**`GET /api/permissions/roles/{name}`** — returns scopes:

```json
{
  "name": "devel-builder",
  "description": "Builder for ces-devel channels",
  "builtin": false,
  "caps": ["builds:create", ...],
  "scopes": [
    { "type": "channel", "pattern": "ces-devel/*" },
    { "type": "registry",
      "pattern": "harbor.clyso.com/ces-devel/*" }
  ],
  "created_at": 1711900000
}
```

**`PUT /api/permissions/roles/{name}`** — update caps and scopes:

Same body as `POST`. Replaces both caps and scopes atomically.

### User assignment endpoints (scopes removed)

**`PUT /api/permissions/users/{email}/roles`** — simplified:

```json
{
  "roles": ["devel-builder", "viewer"]
}
```

Previously each entry was `{ "role": "...", "scopes": [...] }`.
Now it is a flat list of role names.

**`POST /api/permissions/users/{email}/roles`** — simplified:

```json
{
  "role": "devel-builder"
}
```

The `scopes` field is removed.

**`GET /api/permissions/users/{email}/roles`** — response
includes role-level scopes (read-only):

```json
[
  {
    "role": "devel-builder",
    "scopes": [
      { "type": "channel", "pattern": "ces-devel/*" }
    ]
  }
]
```

Scopes in the response now come from the role definition,
not from the assignment. The response shape is unchanged —
the `cbc` client's `admin users get` output continues to
display scopes under each role without modification to the
display logic.

### Scope-dependent capability validation

The existing validation ("roles with scope-dependent caps must have
scopes") moves from **assignment time** to **role definition time**.
When creating or updating a role that includes `builds:create` (or
any cap in `SCOPE_DEPENDENT_CAPS`), the API requires at least one
scope entry on the role — unless the role also has `*` (admin
wildcard).

## Implementation Scope

### Schema changes

- Remove `user_role_scopes` table from
  `migrations/001_initial_schema.sql`
- Add `role_scopes` table to the same migration

### DB layer (`db/roles.rs`)

- Add `set_role_scopes()`, `get_role_scopes()`
- Simplify `set_user_roles()` — remove scope insertion; accept
  a `&[&str]` of role names instead of `&[RoleAssignment]`
- Simplify `get_user_roles()` — return role names only (or
  read-through role scopes for display)
- Update `get_user_assignments_with_scopes()` — join
  `role_scopes` instead of `user_role_scopes`
- Remove `RoleAssignment.scopes` field; `ScopeEntry` struct
  remains (used by role scopes)

### Scope matching consolidation

Two independent `scope_pattern_matches` functions currently exist
(`auth/extractors.rs` and `channels/mod.rs`). Consolidate them
into a single shared function — either in `cbsd-proto` (if it
should be available to both server and worker) or in a shared
`auth` utility module within `cbsd-server`.

### Auth extractor (`auth/extractors.rs`)

- No changes to `require_scopes_all()` — transparent
- Remove local `scope_pattern_matches`, use shared version

### Route handlers

- `routes/permissions.rs` — add scopes to role CRUD bodies and
  responses; remove scopes from user assignment bodies; move
  scope-dependent validation to role creation/update; add
  API-layer validation of `scope_type` values against the known
  set (`channel`, `registry`, `repository`) so that invalid
  types return a clean 400 instead of a database error
- `routes/builds.rs` — no change
- `routes/periodic.rs` — fix `validate_descriptor_scopes` to
  use the `channel/type` composite format (or skip the channel
  scope check when the type is not yet known, since
  `resolve_and_rewrite` performs its own channel scope check
  downstream via enforcement point 4)
- `routes/channels.rs` — refactor `user_can_view_channel()` to
  use the shared `scope_pattern_matches` function instead of
  hand-rolled inverted matching
- `channels/mod.rs` — remove local `scope_pattern_matches`, use
  shared version

### Seed (`db/seed.rs`)

- `admin` — no scopes (wildcard cap, global)
- `builder` — seed with `channel=*`, `repository=*` to preserve
  current "global builder" default behaviour
- `viewer` — no scopes (no scope-dependent caps)

**Implementation note:** currently, the builder role has no seeded
scopes — users assigned with empty scopes get global access via
the `scopes.is_empty()` short-circuit. After migration, builder
has explicit `*` patterns, so `scopes.is_empty()` returns `false`
and the pattern-matching path runs instead. Both paths produce
the same result, but all three `scopes.is_empty()` sites must be
audited during implementation to confirm correctness with wildcard
patterns:

1. `auth/extractors.rs` — `require_scopes_all()`
2. `channels/mod.rs` — `check_channel_scope()`
3. `routes/channels.rs` — `user_can_view_channel()`

### cbc client (`cbc/`)

Scopes move from user-role assignment commands to role
management commands. Two files are affected:

**`cbc/src/admin/roles.rs`** — add scope support:

- `CreateArgs` — add `--scope TYPE=PATTERN` (repeatable)
- `UpdateArgs` — add `--scope TYPE=PATTERN` (repeatable)
- `CreateRoleBody`, `UpdateRoleBody` — add
  `scopes: Vec<ScopeItem>` field
- `RoleDetail` — add `scopes: Vec<ScopeItem>` for GET
  response deserialization
- `cmd_create`, `cmd_update` — parse `--scope` args into
  request body
- `cmd_get` — display scopes alongside capabilities
- Add `ScopeItem` struct and `parse_scope()` function
  (moved from `users.rs`)

**`cbc/src/admin/users.rs`** — remove scope support:

- `RolesSetArgs` — simplify `--role` to plain role names;
  remove `NAME:TYPE=PAT[,TYPE=PAT,...]` inline syntax
- `RolesAddArgs` — remove `--scope` flag entirely
- `ReplaceUserRolesBody` — change from
  `{ "roles": [{ "role": "...", "scopes": [...] }] }`
  to `{ "roles": ["name1", "name2"] }` (flat list)
- `AddUserRoleBody` — remove `scopes` field
- Delete `RoleAssignment` struct
- Delete `parse_role_spec()` and `parse_scope()` (scope
  parsing moves to `roles.rs`)
- `cmd_roles_set`, `cmd_roles_add` — simplify to send
  role names only

**Display changes:**

- `admin roles get` — add scopes to output (currently
  shows only name, builtin, description, capabilities)
- `admin users get` — scopes are still visible under each
  role (the server response includes role-level scopes),
  but the `UserRoleItem` response struct reflects the new
  shape (scopes come from the role definition)

**CLI usage (before → after):**

```
# Before: scopes specified at assignment time
cbc admin roles create prod-builder \
    --cap builds:create
cbc admin users roles add alice@clyso \
    --role prod-builder \
    --scope "channel=ces-prod/*"

# After: scopes specified at role definition time
cbc admin roles create prod-builder \
    --cap builds:create \
    --scope "channel=ces-prod/*"
cbc admin users roles add alice@clyso \
    --role prod-builder
```

### Offline query cache

- Regenerate `.sqlx/` after migration and query changes

## Migration Note

No migration path is provided. The `001_initial_schema.sql` is
rewritten in place. Existing databases must be recreated. This is
acceptable at the current stage of the project.

[003]: 003-20260313T2129-cbsd-auth-permissions-design.md
