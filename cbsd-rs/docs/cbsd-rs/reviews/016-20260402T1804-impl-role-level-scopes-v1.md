# 016 — Role-Level Scopes: Implementation Review v1

**Design:**
`docs/cbsd-rs/design/016-20260402T1200-role-level-scopes.md`

**Plan:**
`docs/cbsd-rs/plans/016-20260402T1600-role-level-scopes.md`

**Commits reviewed:** `a8fca90..3aa201f` (3 code commits)

## Summary

The implementation faithfully follows the design and plan.
All four planned commits were executed. The schema change,
scope consolidation, enforcement-point fixes, and cbc client
adaptation all land in the correct order with the correct
dependencies. The plan's progress table shows all four
commits as Done.

## Findings

### F1 (Medium) — `ScopeBody` / `ScopeEntry` conversion boilerplate

`permissions.rs` contains **7 occurrences** of the same
`ScopeBody` <-> `db::roles::ScopeEntry` mapping (lines 298,
387, 466, 625, 670, 741, 805). Both structs have identical
fields (`scope_type`, `pattern`). This is the single largest
source of repetitive code in the diff.

**Options:**

- Add `From<ScopeBody> for ScopeEntry` and
  `From<ScopeEntry> for ScopeBody` impls on the route-layer
  types so each conversion site becomes `.map(Into::into)`.
- Alternatively, make `ScopeEntry` derive `Serialize` +
  `Deserialize` with `#[serde(rename = "type")]` on the
  `scope_type` field and use it directly as the wire type,
  eliminating `ScopeBody` entirely (the route module already
  imports `db::roles`).

Either approach removes ~40 lines of mechanical mapping.

### F2 (Low) — `update_role` rollback does not restore scopes

`permissions.rs:481-489`: when the last-admin guard fires
after a `PUT /roles/{name}`, the rollback restores old caps
via `set_role_caps` but does **not** restore old scopes.
The scopes have already been replaced at line 471. If the
guard fires, the role ends up with old caps but new scopes.

This is inherited from the pre-existing caps-only rollback
pattern and is low-severity (the guard only fires when
removing `*` from the last admin role, an uncommon edge
case), but it is now a wider gap because scopes are also
being replaced.

**Suggested fix:** save old scopes before the update (like
`old_caps`), and restore them alongside caps in the rollback
branch. Or wrap the entire update (caps + scopes) in a
single DB transaction so the rollback is atomic.

### F3 (Low) — `set_role_scopes` + `set_role_caps` are separate transactions

`create_role` (line 284-310) and `update_role` (line 451-478)
call `set_role_caps` and `set_role_scopes` as two independent
transactions. If the server crashes between the two calls,
the role ends up with caps but no scopes (or vice versa).

This is pre-existing (caps were already set in a separate
transaction from role creation), but the addition of scopes
doubles the window. Consider combining them into a single
transaction in a future cleanup.

### F4 (Low) — N+1 queries in `get_user_roles` and `get_user_assignments_with_scopes`

`db/roles.rs:291-310` (`get_user_roles`) and
`db/roles.rs:336-369` (`get_user_assignments_with_scopes`)
each issue one `get_role_scopes` query per role in a loop.
This replaces the previous N+1 pattern (which queried
`user_role_scopes` per role), so it is not a regression —
but now that scopes live on roles rather than per-assignment,
a single JOIN query could fetch all roles + scopes in one
round trip:

```sql
SELECT ur.role_name, rs.scope_type, rs.pattern
FROM user_roles ur
LEFT JOIN role_scopes rs ON ur.role_name = rs.role_name
WHERE ur.user_email = ?
ORDER BY ur.role_name, rs.scope_type, rs.pattern
```

This would eliminate the loop and reduce DB round trips.
Not urgent — the role count per user is small — but worth
noting for future optimization.

### F5 (Informational) — `scope_covers_channel` allocates on every call

`scopes.rs:47`: `scope_covers_channel` creates a
`format!("{channel_name}/")` `String` on every invocation.
This is fine for correctness and the call rate is low, but
if it ever appears in a hot path, it could be replaced with
a comparison that avoids allocation:

```rust
pattern.len() > channel_name.len()
    && pattern.as_bytes()[channel_name.len()] == b'/'
    && pattern.starts_with(channel_name)
```

No action needed now.

### F6 (Informational) — `CreateRoleBody` reused for PUT handler

`update_role` (line 404) deserializes the PUT body as
`Json<CreateRoleBody>`. This works because the field sets
happen to be identical, but semantically a PUT update
should not require the `name` field in the body (the name
comes from the path parameter). The `name` field in the
body is silently ignored. The plan specified a separate
`UpdateRoleBody` for the cbc client but not for the server.
This is cosmetic — the current approach is functional.

## Design Conformance

| Design requirement | Status | Notes |
|--------------------|--------|-------|
| `user_role_scopes` removed | Done | Migration rewritten |
| `role_scopes` table added | Done | Correct schema, CHECK constraint |
| `set_role_scopes`, `get_role_scopes` | Done | Plus `_in_tx` variant for seed |
| `set_user_roles` simplified | Done | Accepts `&[&str]` |
| `get_user_roles` reads role scopes | Done | Via `get_role_scopes` |
| `get_user_assignments_with_scopes` updated | Done | Via `get_role_scopes` |
| `RoleAssignment` struct deleted | Done | |
| Scope-dependent validation moved to role CRUD | Done | `create_role`, `update_role` |
| `validate_scope_type` (F5 from design review) | Done | `KNOWN_SCOPE_TYPES` const |
| Role CRUD includes scopes | Done | Create, Get, Update |
| User assignment endpoints simplified | Done | Flat role name list |
| `scope_pattern_matches` consolidated | Done | `scopes.rs` shared module |
| `scope_covers_channel` helper | Done | With tests |
| `periodic.rs` channel scope bug fixed | Done | Raw check removed |
| `channels.rs` visibility refactored | Done | Uses `scope_covers_channel` |
| Seed: builder gets `channel=*`, `repository=*` | Done | Via `set_role_scopes_in_tx` |
| Seed: admin has no scopes | Done | Unchanged |
| Seed: viewer has no scopes | Done | Unchanged |
| `scopes.is_empty()` audit | Done | Confirmed in plan; no code change needed |
| cbc: scopes on role create/update | Done | `--scope` flag |
| cbc: scopes removed from user commands | Done | `--scope` and inline syntax removed |
| cbc: `admin roles get` shows scopes | Done | Aligned display format |
| `.sqlx/` cache regenerated | Done | 4 JSON files changed |

## Plan Conformance

| # | Planned commit | Actual commit | Match |
|---|---------------|---------------|-------|
| 1 | docs | `a8fca90` (pre-range) | Yes |
| 2 | scope matching + bug fixes | `194cf6d` | Yes |
| 3 | server role-level scopes | `a1503ec` | Yes |
| 4 | cbc client | `3aa201f` | Yes |

All four commits land in the planned order with the
planned content.

## Deferred / Skipped Work

Nothing was deferred or skipped. Every item from the
design's "Implementation Scope" section and every item
from the plan's commit breakdown is accounted for in the
diff.

The design explicitly noted that `ScopeType::Registry`
is defined but has no active enforcement points. This
remains unchanged and is not deferred work — it was
explicitly out of scope.

## Verdict

The implementation is complete and correct against the
design and plan. F1 (conversion boilerplate) is the main
actionable item for a follow-up cleanup. F2 (rollback gap)
is worth addressing but is low-risk.
