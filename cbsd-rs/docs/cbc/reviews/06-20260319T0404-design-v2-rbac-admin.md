# Design Review: 06 — RBAC Administration (v2)

**Verdict: Revise and re-review.**

Both v1 blockers resolved: scopes fully documented with
`--scope type=pattern`, `--force` on `roles delete`, all
error cases covered. However, **2 wire-format bugs** remain
that will cause runtime failures.

## Blockers

### B1 — `users roles set` body field is `assignments`, server expects `roles`

The design shows:

```json
{"assignments": [{"role": "builder", "scopes": [...]}]}
```

The server's `ReplaceUserRolesBody` has field `roles`:

```rust
struct ReplaceUserRolesBody {
    roles: Vec<RoleAssignmentBody>,
}
```

Sending `"assignments"` will deserialize as empty `roles`,
silently removing all user roles. This is a data-destructive
wire-format bug.

**Fix:** Change `"assignments"` to `"roles"` in the request
body example.

### B2 — Scope JSON field is `"type"`, not `"scope_type"`

The design shows:

```json
{"scope_type": "channel", "pattern": "ces-devel"}
```

The server's `ScopeBody` has `#[serde(rename = "type")]` on
the `scope_type` field — the wire format is:

```json
{"type": "channel", "pattern": "ces-devel"}
```

Sending `"scope_type"` will be ignored by serde (unknown
field), producing scope objects with no type — the server
may accept them as empty or reject with a validation error.

**Fix:** Change `"scope_type"` to `"type"` in all scope JSON
examples.

## Major Concerns

### M1 — `admin users get` doesn't need two requests

The design says two requests are needed: (1) list all users,
(2) get roles for one user. But `GET /api/permissions/users`
already returns `Vec<UserWithRolesItem>` which includes
`email`, `name`, `active`, AND `roles` (with scopes). A
single list request filtered client-side is sufficient.

**Fix:** Simplify to one request: `GET /api/permissions/users`,
filter by email. This eliminates the second request entirely.

## Minor Issues

- **`roles list` correctly drops caps column.** The server's
  `RoleListItem` has no caps — showing them only in
  `roles get` is the right call.

- **`admin:queue:view` capability documented.** Good.

- **Builtin role update → 409 documented.** Good.

- **Last-admin guard on `roles delete` and `roles update`
  documented.** Good.

- **Role assignment not found → 404 documented.** Good.

- **Scope-dependent role without scopes → 400 documented.**
  Good.

## Strengths

- Scope model is now fully documented with `--scope`
  flag, scope types, and the scope-dependent capability
  enforcement.
- `--force` on `roles delete` correctly maps to `?force=true`.
- Error handling section is comprehensive (9 cases).
- `roles update` notes `name` must match path parameter.
- `users list` notes scopes omitted with pointer to
  `users get`.
- `users get` shows full role + scope detail view.
