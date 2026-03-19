# Design Review: 06 — RBAC Administration (v3)

**Verdict: Approved.**

Both v2 blockers resolved:

- B1: Request body field is now `"roles"` (not
  `"assignments"`). Matches `ReplaceUserRolesBody`.
- B2: Scope JSON uses `"type"` (not `"scope_type"`).
  Matches `ScopeBody` with `#[serde(rename = "type")]`.

The v2 major concern is also resolved:
- `admin users get` now uses a single request
  (`GET /api/permissions/users`, filter client-side).

No blockers. No major concerns.

## Minor Issues

- The `effective caps` line in `users get` output is still
  shown but the design doesn't specify how it's derived.
  The single-request `GET /api/permissions/users` does not
  return effective caps — those require per-role cap
  lookups. Either drop the caps line or accept the N+1
  cost (acceptable given small role count per user).

## Strengths

- Wire-format JSON examples are now correct for both
  `roles` and `type` fields.
- Single-request implementation for `users get` eliminates
  unnecessary complexity.
- Scope model is fully documented with types, patterns,
  and enforcement.
- Error handling covers all 8 cases including scope
  validation, assignment-not-found, and builtin update.
- `--force` on `roles delete` maps to `?force=true`.
- `admin:queue:view` capability documented.
- Last-admin guard documented for all three paths
  (deactivate, roles delete, roles update).
