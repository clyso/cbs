# 014 — Plan Review: Channel/Type Mapping (v1)

**Plan:**
`docs/cbsd-rs/plans/014-20260324T0037-channel-type-mapping.md`

**Design:**
`docs/cbsd-rs/design/014-20260323T2132-channel-type-mapping.md`
(v3, approved)

**Verdict:** One compilation-breaking issue; otherwise
sound

---

## Findings

### F1 — Important: commit 3 breaks compilation

Commit 3 changes `BuildDescriptor.channel` from
`String` to `Option<String>` in `cbsd-proto` and lists
only `cbsd-proto/src/build.rs` (~20 lines).

This breaks every consumer of `descriptor.channel`.
Verified 5 call sites across 4 files:

| File | Line | Usage |
|------|------|-------|
| `cbc/src/builds.rs` | 248 | `channel: args.descriptor.channel.clone()` — now needs `Some(...)` |
| `cbc/src/builds.rs` | 283 | Display — now `Option`, needs `.as_deref().unwrap_or("")` or similar |
| `cbc/src/periodic.rs` | 285 | Same as line 248 |
| `cbsd-server/src/scheduler/tag_format.rs` | 131 | `Some(descriptor.channel.clone())` — now `Some(Option<String>)`, double-wrapped |
| `cbsd-server/src/routes/builds.rs` | 100 | `body.descriptor.channel.clone()` — pushed into `Vec<(ScopeType, String)>`, type mismatch |

Plus 2 test sites in `cbsd-proto` itself (build.rs:151,
ws.rs:150).

The golden rule: every commit must compile. Commit 3
must also fix all consumers, or be merged into a
commit that does.

**Options:**

**(a)** Expand commit 3 to fix all call sites with
minimal bridging (e.g., `.unwrap_or_default()`,
`Some(...)` wrappers). This keeps the proto change
isolated but the commit grows to ~50-80 lines across
5 files. Rename to something like `cbsd-rs: make
channel optional in BuildDescriptor`.

**(b)** Merge commit 3 into commit 5 (build submission
resolution). This is where the channel field is
properly handled with the new resolution logic. But it
makes commit 5 larger (~320+ lines) and conflates two
concerns (proto change + feature logic).

Option (a) is cleaner — the bridging fixes are
trivial (add `Some()` at construction sites,
`.unwrap_or_default()` at consumption sites) and the
commit remains a coherent "make channel optional"
change.

### F2 — Moderate: resolution logic placement

Commit 5 adds channel/type resolution to
`routes/builds.rs` (the `submit_build` handler).
Commit 6 adds it to `scheduler/trigger.rs`.

Currently, `trigger_periodic_build` calls
`insert_build_internal` directly (trigger.rs:100),
bypassing `submit_build`. If the resolution logic
lives only in `submit_build`, it must be duplicated
in the trigger — or extracted into a shared helper.

The plan should specify:

- **Shared helper** (preferred): extract a
  `resolve_channel_type(state, channel, type, email)`
  function in a module both paths can call. This
  avoids duplication and keeps the resolution logic
  in one place.
- **In `insert_build_internal`**: move resolution
  into the shared function. But this couples the
  shared function to channel resolution, which may
  not be desirable for all callers.
- **Duplicated**: resolution in both submit_build and
  trigger. Fragile — changes must be synchronized.

### F3 — Moderate: periodic trigger scope checking

The current trigger validates the user is active and
has `builds:create` but does **not** check scopes
(it calls `insert_build_internal` directly, which
doesn't check scopes). The design says "the same
resolution logic applies at trigger time."

Does "same logic" include scope re-validation? If an
admin revokes a user's `ces/release` scope, should
their existing periodic task for `ces/release` fail
at the next trigger?

The plan should state one of:
- **Yes, re-check scopes** — more secure, prevents
  stale permissions from producing builds
- **No, skip scopes** — the task was authorized at
  creation time; revoking scopes requires manually
  disabling the task

Both are valid but the plan should be explicit.

### F4 — Minor: commit 4 missing `permissions.rs`

Commit 4 adds `channels:manage` and `channels:view`
"to builtin admin role caps" in seed.rs. The admin
role already has `*` (wildcard). What actually matters
is adding the new capability names to `KNOWN_CAPS` in
`routes/permissions.rs:26-43`, which validates
capabilities at the API layer.

Without this, an admin trying to assign
`channels:manage` to a custom role would get
"unknown capability" rejected.

**Fix:** Add `permissions.rs` to commit 4's file
list.

### F5 — Minor: `extractors.rs` in commit 5

Commit 5 lists `auth/extractors.rs` for "scope
validation for channel/type format." The existing
`scope_pattern_matches` already handles the new
format correctly:

- `ces/dev` → exact match ✓
- `ces/*` → prefix match ✓
- `*` → matches all ✓

The actual changes are:
- `routes/builds.rs`: construct scope check value as
  `"{channel_name}/{type_name}"` instead of raw
  `descriptor.channel`
- `routes/permissions.rs`: validate `/` in channel
  scope patterns

`extractors.rs` may not need changes at all. The
plan should verify and adjust the file list if so.

---

## What's Sound

**Commit ordering.** The dependency chain is correct:
schema → proto → endpoints → resolution → scheduler
→ client → docs.

**Split points.** Commit 4 (~500 lines) has a
documented split strategy. Commit 5 (~300 lines) is
correctly kept as one unit. Commit 6 (~50 lines) is
appropriately small.

**Sizing.** Most commits are within the 400-800 line
guideline. Commits 3 and 6 are small but justified —
they're distinct logical changes.

**Implementation notes.** The N1-N3 decisions from
the design review are resolved clearly at the top.

---

## Summary

| Severity | # | Finding |
|----------|---|---------|
| Important | 1 | F1: commit 3 breaks compilation |
| Moderate | 2 | F2: resolution helper vs duplication; F3: periodic scope checking |
| Minor | 2 | F4: missing permissions.rs; F5: extractors.rs may be unnecessary |

F1 must be fixed before implementation — it violates
the compilability invariant. F2-F3 are design
questions the plan should answer. F4-F5 are file list
corrections.
