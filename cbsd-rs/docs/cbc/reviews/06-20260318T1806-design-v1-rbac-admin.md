# Design Review: 06 — RBAC Administration

**Document:** `docs/cbc/design/06-20260318T1806-rbac-admin.md`

---

## Summary

The scope model — the most important part of RBAC — is almost
entirely absent. Every role assignment endpoint carries
`scopes: Vec<ScopeBody>`, and scope-dependent roles (`builder`)
require scopes at assignment time. The design documents none of
this. Additionally, `roles delete` is missing the `--force` flag
required by the server. This document needs significant revision.

**Verdict: Revise and re-review.**

---

## Blockers

### B1 — Scope model completely absent

The server's `AddUserRoleBody` requires `scopes: Vec<ScopeBody>`
where each scope is `{type: "channel", pattern: "ces-devel"}`.
For roles with `builds:create`, the server enforces that scopes
are non-empty (returns 400 otherwise).

The design's `admin users roles add` sends `{"role": "builder"}`
with no scopes. Every call for the `builder` role will fail with
400: "role 'builder' contains scope-dependent capabilities and
requires scopes."

**Fix:** Add `--scope <type>=<pattern>` flag (repeatable) to
`users roles add` and `users roles set`. Document scope types
(`channel`, `registry`, `repository`). Note that the `builder`
role requires at least one scope.

### B2 — `roles delete` missing `--force` flag

The server's `delete_role` returns 409 if the role has active
user assignments unless `?force=true` is passed. The design has
no `--force` flag.

**Fix:** Add `--force` flag to `admin roles delete` that appends
`?force=true` to the request.

---

## Major Concerns

### M1 — `admin users get` requires multiple requests

The design says the endpoint is
`GET /api/permissions/users/{email}/roles`. That returns
`Vec<UserRoleItem>` (role names + scopes) — not email, name,
or active status. There is no single-user GET endpoint.

To produce the design's output (email, name, active, roles,
caps), the client needs:
1. `GET /api/permissions/users` (list all, filter by email).
2. `GET /api/permissions/users/{email}/roles` (role list).
3. N × `GET /api/permissions/roles/{name}` (caps per role).

**Fix:** Document the multi-request pattern. Accept the N+1
cost given the small number of roles per user.

### M2 — `roles list` shows caps but the API doesn't return them

The server's `RoleListItem` has `name`, `description`,
`builtin`, `created_at` — no `caps` field. To show caps in
the list, the client needs N extra requests (one per role).

**Fix:** Either drop caps from the list view (show only in
`roles get`) or document the N+1 pattern.

### M3 — `admin queue` capability not documented

The server requires `admin:queue:view`. The design doesn't
mention the capability requirement. A user who tries
`admin queue` and gets 403 has no guidance.

**Fix:** Add "Requires: `admin:queue:view`."

### M4 — `roles update` for builtin roles returns 409

The server returns 409 "cannot modify a builtin role" on PUT
for builtin roles. The design's error section covers delete
of builtins but not update. Add this case.

---

## Minor Issues

- **`users list` output omits scopes.** The server returns
  roles with scopes. Add a note: "Scopes omitted from list
  view — use `users get` for full details."

- **`roles delete` builtin error is 409, not 400.** The design
  says "server returns 400" — correct it to 409 CONFLICT.

- **`users roles remove` — 404 message is "role assignment not
  found".** The design says "role not found" and "user not
  found" — add the assignment-not-found case.

- **`roles update` body must include `name: String`.** The
  server's `CreateRoleBody` requires `name` (not optional).
  The client must send it matching the path.

- **Last-admin guard fires on `roles delete` and `roles update`
  too.** The design documents it only for `users deactivate`.

- **`roles list` doesn't show `description` column.** Available
  in `RoleListItem` — useful for custom roles.

---

## Suggestions

- **`admin roles caps`** command listing all known capabilities
  with descriptions — useful for operators building custom
  roles.

- **`--watch` flag on `admin queue`** for periodic polling.

- **Pre-validate role existence** in `users roles set` before
  sending the PUT, for better error messages.

---

## Strengths

- Three-tier command tree (`admin roles`, `admin users`,
  `admin users roles`) maps well to the server's API structure.
- `roles update` semantics explicitly documented as "replace
  all" (not additive).
- Error handling section is the most complete of all 7 docs.
- `admin queue` output maps directly to the server's JSON
  structure.
