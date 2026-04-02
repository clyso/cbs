# Design Review: Role-Level Scopes (v1)

**Document:** `016-20260402T1200-role-level-scopes.md`
**Reviewer:** Claude
**Date:** 2026-04-02

## Verdict: Approve with Findings

The design is sound and the motivation is well-argued. All factual
claims about the current codebase were validated against source. One
enforcement point is missing from the enumeration, and there are a
few design-level observations worth addressing before implementation.

## Findings

### F1 — Missing enforcement point (Medium)

**Claim (doc line 136):** "Three places in the codebase gate access
using scopes."

**Actual:** There are **four**. The doc omits:

- `cbsd-server/src/channels/mod.rs:153` —
  `check_channel_scope()`, called from `resolve_and_rewrite()`.

This function calls `get_user_assignments_with_scopes()` directly
and checks Channel scopes, exactly like enforcement point 3 in
`channels.rs`. It is invoked from two paths:

1. `routes/builds.rs:124` — build submission (after the Repository
   scope check at line 107, so the same request hits **two**
   independent scope gates).
2. `scheduler/trigger.rs:102` — periodic build trigger execution.

The "Change needed: None" conclusion is still correct — the
function is transparent to the scope data source — but the doc
should enumerate it for completeness. Anyone auditing enforcement
points from this document alone would miss it.

**Recommendation:** Add a fourth enforcement point section for
`channels/mod.rs::check_channel_scope()`, noting that it is
reached via `resolve_and_rewrite()` in both the build submission
and scheduler trigger paths.

---

### F2 — Channel scope check in `periodic.rs` uses raw channel name, not channel/type (Low)

The doc says enforcement point 2 (`validate_descriptor_scopes`)
checks Channel scopes. This is accurate. However, this function
checks the **raw** `descriptor.channel` value (e.g., `"ces-devel"`)
against scope patterns, while the `check_channel_scope()` in
`channels/mod.rs` checks the **resolved** `channel/type` composite
(e.g., `"ces-devel/dev"`).

This means the two Channel scope checks use different value
formats. This is a pre-existing inconsistency, not introduced by
this design, but the migration provides an opportunity to document
or reconcile it. With role-level scopes, a pattern like
`ces-devel/*` would match both formats, so the practical impact is
low — but a pattern like `ces-devel/dev` would match the
`channels/mod.rs` check but not the `periodic.rs` check (which
sees only `"ces-devel"`).

**Recommendation:** Note this pre-existing inconsistency in the
design doc or flag it for future harmonization.

---

### F3 — Flexibility trade-off not discussed (Low)

The design correctly identifies the scaling benefit (one role
update vs N assignment updates). It does not discuss the
corresponding loss: per-user scope customization within the same
role is no longer possible. If Alice needs `ces-devel/*` +
`ces-staging/*` but Bob needs only `ces-devel/*`, two distinct
roles are required.

This is clearly the intended model (the example shows
`devel-builder` vs `prod-builder`), and it is the right trade-off
for this project. Acknowledging it in the design would
pre-empt future questions about "why can't I add a scope just
for this one user?"

**Recommendation:** Add a brief note in the Design section
acknowledging that per-user scope overrides are intentionally
removed and that distinct roles are the expected mechanism.

---

### F4 — Seed builder scopes vs. current behavior (Low)

The doc proposes seeding `builder` with `channel=*`,
`repository=*`. Currently, `builder` is seeded with no scopes at
all (`db/seed.rs:95-109`), and `set_user_roles()` inserts scopes
per assignment — meaning a builder assigned with no scopes gets
global access (empty scopes = global). After migration, the
`builder` role with `channel=*`, `repository=*` also gets global
access via wildcard match.

The end behavior is equivalent, but the mechanism differs (empty
list = global vs. wildcard pattern = global). This is fine, but
worth noting that future code checking `scopes.is_empty()` as a
proxy for "global access" (as `check_channel_scope` does at
`channels/mod.rs:164`) would behave differently after the seed
change. The builder role would no longer have empty scopes — it
would have `*` patterns.

**Recommendation:** Audit all `scopes.is_empty()` checks to
confirm they still produce correct results when a role has
explicit `*` patterns instead of an empty scope list. The
`require_scopes_all()` method and `check_channel_scope()` both
short-circuit on empty scopes — with `*` patterns, they would
fall through to the pattern-matching path (which would still
match everything, but via a different code path).

---

### F5 — Atomicity of role update (Informational)

The doc says `PUT /api/permissions/roles/{name}` "replaces both
caps and scopes atomically." This requires a transaction that
deletes old `role_caps` + `role_scopes` rows and inserts new
ones. The current `create_role()` in `db/roles.rs` already uses
a transaction for caps. The implementation plan should ensure the
scope replacement is included in the same transaction.

No design change needed — just a note for the implementation
plan.

## Validated Claims

All factual claims were checked against source code:

| # | Claim | Status |
|---|-------|--------|
| 1 | `user_role_scopes` table exists with per-assignment scopes | Confirmed (`migrations/001_initial_schema.sql:76-85`) |
| 2 | `user_roles` is `(user_email, role_name)` with no scope columns | Confirmed (`migrations/001_initial_schema.sql:69-73`) |
| 3 | `AuthUser::require_scopes_all()` is the main scope gate | Confirmed (`auth/extractors.rs:83-115`) |
| 4 | `get_user_assignments_with_scopes()` joins `user_role_scopes` | Confirmed (`db/roles.rs:329-338`) |
| 5 | `AssignmentWithScopes` struct exists | Confirmed (`db/roles.rs:45-51`) |
| 6 | `set_user_roles()` accepts `&[RoleAssignment]` | Confirmed (`db/roles.rs:199-203`) |
| 7 | `ScopeType::Registry` defined but never enforced | Confirmed (defined at `auth/extractors.rs:34`, no enforcement found) |
| 8 | `SCOPE_DEPENDENT_CAPS` = `["builds:create"]` | Confirmed (`routes/permissions.rs:47`) |
| 9 | Seed roles: admin (`*`), builder (7 caps), viewer (2 caps) | Confirmed (`db/seed.rs:93-117`) |
| 10 | Design 003 exists and covers scopes | Confirmed (`docs/cbsd-rs/design/003-20260313T2129-cbsd-auth-permissions-design.md`) |
| 11 | Current role API has no scopes; user assignment API has scopes | Confirmed (`routes/permissions.rs`) |
| 12 | Enforcement in `builds.rs`, `periodic.rs`, `channels.rs` (visibility) | Confirmed (all three exist at documented locations) |

## Summary

The design is well-structured, the data model change is clean,
and the migration approach (rewrite `001_initial_schema.sql`,
recreate DB) is appropriate for the project stage. The core
algorithm is unchanged — only the data source moves.

**Action items before implementation:**

1. **(F1)** Add `channels/mod.rs::check_channel_scope()` as a
   fourth enforcement point.
2. **(F4)** Audit `scopes.is_empty()` short-circuits against
   the new seed behavior (`*` patterns vs. empty list).
3. **(F2, F3)** Optional: add notes on the channel value format
   inconsistency and the flexibility trade-off.
