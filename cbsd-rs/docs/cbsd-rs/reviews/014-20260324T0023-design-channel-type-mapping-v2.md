# 014 — Design Review: Channel/Type Image Destination Mapping (v2)

**Design:**
`docs/cbsd-rs/design/014-20260323T2132-channel-type-mapping.md`
(v2)

**Prior review:**
`014-20260323T2144-design-channel-type-mapping-v1`

**Verdict:** Two important issues remain; v1 findings
addressed

---

## v1 Finding Resolution

| # | Finding | Status |
|---|---------|--------|
| F1 | Double-registry in image path | Fixed — `dst_image.name` now excludes registry; registry deferred to cbscore config |
| F2 | VersionType enum vs type_name | Fixed — CHECK constraint restricts to four enum values; explicitly no custom types |
| F3 | Channel name migration | Addressed — fresh deployment, no existing data to migrate |
| F4 | default_type_id soft-delete | Fixed — application clears default_type_id on type soft-delete |
| F5 | Seed data not specified | Addressed — no seed; admin creates channels via API post-startup |
| F6 | Build responses need names | Fixed — denormalized channel/type names JOINed into responses |
| F7 | Periodic builds not addressed | Fixed — new section: logical names, resolution at trigger time |
| F8 | Multi-segment image names | Documented |

All v1 findings are resolved or addressed with clear
design decisions.

---

## New Findings

### F1 — Important: registry scope checking breaks

The current build submission code
(`routes/builds.rs:102-105`) extracts a "registry host"
from `dst_image.name` using `registry_host()`, which
returns the first segment before `/`:

```rust
if let Some(host) = body.descriptor.registry_host() {
    scope_checks.push(
        (ScopeType::Registry, host.to_string())
    );
}
```

With the new `dst_image.name = "cbs-internal/joao.luis/
ceph"`, `registry_host()` returns `"cbs-internal"` —
a project name, not a registry host. This value would
be checked against `registry` scope patterns (e.g.,
`"harbor.clyso.com"`). The check fails for any user
with explicit registry scopes.

The design says "registry and repository scope types
remain for potential future use" but does not address
the fact that the current code actively checks
registry scopes at build submission. The implementation
would need to **remove the registry scope check** from
`submit_build`, since the registry is no longer in the
descriptor.

**Suggestion:** Add to the design: "The registry scope
check in `submit_build` is removed. The `registry`
scope type remains in the schema for future use but is
no longer checked at build submission." Also remove
or deprecate `registry_host()` on `BuildDescriptor`.

### F2 — Important: backwards compat doesn't match

The design says:

> Existing scope patterns without a `/` (e.g., `"ces"`)
> are treated as `"ces/*"`.

The new scope check value for a build is
`"{channel}/{type}"` (e.g., `"ces/dev"`). The existing
`scope_pattern_matches` function (`extractors.rs:
114-120`) does exact match for patterns without `*`:

```rust
fn scope_pattern_matches(
    pattern: &str, value: &str,
) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value  // "ces" != "ces/dev"
    }
}
```

Pattern `"ces"` does NOT match value `"ces/dev"` under
the current logic. The "treated as" conversion doesn't
happen automatically — the pattern matching code needs
a change.

**Options:**

**(a)** Modify `scope_pattern_matches` for the
`channel` scope type: if the pattern doesn't contain
`/`, treat it as `"{pattern}/*"` before matching.
Requires passing the scope type into the function.

**(b)** Normalize patterns at insertion time: when a
`channel` scope is created without `/`, append `/*`
before storing.

**(c)** Since this is a fresh deployment with no
existing data, remove the backwards compat clause
entirely. Require all new channel scope patterns to
use the `channel/type` format.

The design should pick one and document it. If (c),
the backwards compat paragraph should be removed.

### F3 — Moderate: scope check missing from default


flow

The build submission flow (steps 3a-3g) doesn't show
when scope checking happens for the default-channel
case. Only the explicit-channel case (step 4a)
mentions "validate user has scope."

The implementation must check scopes regardless of
whether the channel was explicit or defaulted. The
design should add a scope validation step after
channel/type resolution in step 3, e.g.:

```
3b′. Validate user has scope for "user/dev"
```

### F4 — Moderate: type visibility for regular users

The design says `GET /api/channels` is available to
all authenticated users (line 357-360), but
`GET /api/channels/{id}` (which returns "channel with
types") falls under "admin endpoints" requiring
`channels:manage`.

Regular users need to see which types a channel offers
in order to choose `--type`. Either:

- The list endpoint should include types per channel
  in its response, or
- `GET /api/channels/{id}` should be available to
  users who have channel scope access (not just admins)

`cbc channel list` is much more useful if it shows
types alongside channel names.

---

## What's Improved in v2

**Removed server-side channel config.** v1 had a
`channels:` section in `server.yaml` with
`default-channel` resolved at startup (panics if not
found). v2 moves everything to the DB with admin API
management. This is cleaner — no coupling between
config file and DB state.

**Explicit first-startup flow.** The design now walks
through the admin setup experience with clear error
messages for unconfigured states ("no default channel
assigned — contact your administrator").

**Soft-delete lifecycle.** The application-level
`default_type_id` clearing on type deletion is
specified, with the fallback error message when no
default type exists.

**Registry honesty.** Rather than pretending the server
controls the registry, v2 explicitly defers registry
enforcement with a clear rationale about the
cbscore-level changes needed.

**Periodic builds section.** Logical-name resolution
at trigger time with clear failure behavior for
renamed/deleted channels.

---

## Summary

| Severity | # | Finding |
|----------|---|---------|
| Important | 2 | F1: registry scope check breaks; F2: backwards compat pattern matching |
| Moderate | 2 | F3: scope step in default flow; F4: type visibility for users |

F1 and F2 are implementation-level issues that affect
correctness. F1 would cause scope check failures for
any user with explicit registry scopes. F2 means the
backwards compat guarantee doesn't work without code
changes the design doesn't specify.

Both are straightforward to fix in the design —
likely a paragraph each.
