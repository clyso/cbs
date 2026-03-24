# 014 — Design Review: Channel/Type Image Destination Mapping (v3)

**Design:**
`docs/cbsd-rs/design/014-20260323T2132-channel-type-mapping.md`
(v3)

**Prior reviews:**
`014-20260323T2144-design-channel-type-mapping-v1`,
`014-20260324T0023-design-channel-type-mapping-v2`

**Verdict:** Ready for planning

---

## v2 Finding Resolution

| # | Finding | Status |
|---|---------|--------|
| F1 | Registry scope check breaks | Fixed — check removed; `registry_host()` deprecated; files-changed updated |
| F2 | Backwards compat pattern matching | Fixed — dropped; all channel patterns must contain `/`; fresh deployment |
| F3 | Scope check missing from default flow | Fixed — step 3c added; explicit statement that scopes are checked for all builds |
| F4 | Type visibility for regular users | Fixed — list includes types; detail available to users with any scope for the channel |

All v2 findings resolved cleanly.

---

## Minor Notes for Planning

These are not design flaws — they are implementation
details the plan should address.

### N1 — `channel` field: empty string vs Option

`BuildDescriptor.channel` is currently `pub channel:
String` (required, no serde default). The design says
the client sends `channel: ""` or "absent." If "absent"
is supported, the proto field must become
`Option<String>` with `#[serde(default)]`. If only
empty-string is supported, no proto change is needed
and the server treats `""` as "use default."

The plan should specify which approach and whether the
proto crate changes.

### N2 — Scope pattern validation at creation time

The design says "all channel scope patterns must
contain a `/`." The permission management API
(`routes/permissions.rs`) should validate this at
scope creation time — reject patterns like `"ces"`
that lack a `/` for the `channel` scope type. Without
this, an admin could create a pattern that silently
never matches.

### N3 — Channel list scope filtering

The list endpoint filters channels by the user's scope
access. The implementation needs to determine channel
visibility from patterns like `ces/dev`, `ces/*`, or
`*`. A user with `ces/dev` can see channel "ces"
(they have *some* access to it). The plan should
clarify whether type listing within a channel is also
filtered by scope, or whether any access to a channel
reveals all its types.

---

## Design Quality

The v3 design is complete and internally consistent.
The core model (channel → types → project/prefix) is
well-defined. The boundary between server-controlled
(project, prefix) and user-controlled (image name,
tag) fields is clear. The soft-delete lifecycle,
scope checking, and error cases are specified.

The registry deferral is honest and well-reasoned.
The "no config, API only" approach for channel
management simplifies the startup story. The explicit
scope check in both submission flows (default and
explicit channel) closes the authorization gap.

Ready for an implementation plan.
