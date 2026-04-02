# Design Review: Role-Level Scopes (v2)

**Document:** `016-20260402T1200-role-level-scopes.md`
**Reviewer:** Claude
**Date:** 2026-04-02

## Verdict: Approve with Findings

The design has been revised to address all five v1 findings
(enforcement point 4 added, channel format inconsistency
documented, trade-off section added, seed note added). The core
proposal — moving scopes from user-role assignments to role
definitions — remains sound and well-motivated.

This v2 review found new issues not covered by v1, including one
**high-severity factual error** shared by both the design and the
v1 review.

## Findings

### F1 — Incorrect claim: `ces-devel/*` does NOT match raw channel name (High)

**Claim (doc lines 210–211):** "In practice, glob patterns like
`ces-devel/*` match both formats, so the impact is low."

**Actual:** `scope_pattern_matches("ces-devel/*", "ces-devel")`
returns **false**.

Trace through `auth/extractors.rs:119-125`:

```rust
fn scope_pattern_matches(pattern: &str, value: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
}
```

1. `"ces-devel/*".strip_suffix('*')` → `Some("ces-devel/")`
2. `"ces-devel".starts_with("ces-devel/")` → **false**

The trailing `/` in the stripped prefix means the raw channel
name `"ces-devel"` (no `/type` suffix) does not match.

**Impact:** Enforcement point 2 (`periodic.rs:587-590`) pushes
the raw `descriptor.channel` value (e.g., `"ces-devel"`) into
`require_scopes_all()`. Any user whose only matching scope is
`ces-devel/*` would **fail** the periodic task creation/update
check, even though they are authorized for all types under that
channel.

Only the global wildcard `*` matches both formats:
`strip_suffix('*')` → `Some("")`, and `"ces-devel".starts_with("")`
→ true.

The v1 review (F2, line 60–63) also repeats this incorrect claim.
This is not a "low impact" inconsistency — it is a **functional
bug** for any non-global channel scope pattern in periodic task
validation.

**Recommendation:** Correct the claim in the design. Elevate the
channel format inconsistency from "note for future harmonization"
to a concrete action item: either fix `periodic.rs` to use the
`channel/type` composite (aligning with the other enforcement
points), or adjust `scope_pattern_matches` to handle the raw
format. This should be resolved before or alongside the
role-level scopes migration, since the inconsistency becomes
more visible when scopes are defined once centrally on roles.

---

### F2 — Enforcement point 3 uses different matching logic (Medium)

**Claim (doc lines 174–180):** Enforcement point 3 (channel
visibility) calls `get_user_assignments_with_scopes()` and checks
Channel scopes. "Change needed: None."

**Actual:** `user_can_view_channel()` at `channels.rs:178-188`
does **not** use `scope_pattern_matches()`. It uses hand-rolled
matching:

```rust
a.scopes.iter().any(|s| {
    s.scope_type == ScopeType::Channel.as_str()
        && (s.pattern == "*"
            || s.pattern.starts_with(
                   &format!("{channel_name}/"))
            || s.pattern == format!("{channel_name}/*"))
})
```

This is an **inverted** approach: it checks whether the
**pattern** references the channel name, not whether the pattern
matches a value derived from the channel. The other three
enforcement points all use `scope_pattern_matches(pattern, value)`
in the normal direction.

The practical difference is narrow for currently-validated
patterns (which must contain `/`), but the logic divergence is
a maintenance risk:

- Future changes to `scope_pattern_matches` (e.g., supporting
  `?` or character classes) would not propagate here.
- The asymmetry is invisible to role admins who expect consistent
  matching semantics across all enforcement points.

**Recommendation:** Document this matching asymmetry in the
enforcement point 3 section. Consider flagging consolidation
onto `scope_pattern_matches` as a follow-up, since the migration
is a natural opportunity to align all four enforcement points.

---

### F3 — Implementation note omits `user_can_view_channel` `scopes.is_empty()` (Low)

**Claim (doc lines 336–343):** "all `scopes.is_empty()` checks
must be audited during implementation to confirm correctness with
wildcard patterns." The note names `require_scopes_all()` and
`check_channel_scope()`.

**Actual:** There are **three** `scopes.is_empty()` short-circuit
sites:

1. `auth/extractors.rs:100` — `require_scopes_all()`
2. `channels/mod.rs:164` — `check_channel_scope()`
3. `channels.rs:179` — `user_can_view_channel()`

Site 3 is not mentioned. After migration, the builder role's
scopes change from empty to `[channel=*, repository=*]`. The
`scopes.is_empty()` at site 3 returns false, and the code falls
through to pattern matching. The `s.pattern == "*"` check at
`channels.rs:184` handles this correctly — but the audit note
should enumerate all three sites.

**Recommendation:** Add `user_can_view_channel()` to the
`scopes.is_empty()` audit list.

---

### F4 — Duplicate `scope_pattern_matches` functions (Low)

Two independent `scope_pattern_matches` functions exist:

1. `auth/extractors.rs:119-125` — used by `require_scopes_all()`
2. `channels/mod.rs:179-188` — used by `check_channel_scope()`

They are functionally equivalent (`*` resolves to empty prefix in
both), but the `channels/mod.rs` copy has an explicit `if pattern
== "*"` early return that the `extractors.rs` copy does not.

The implementation scope section does not mention consolidating
them. Two copies of matching logic is a drift risk, especially
when the migration changes the scope data model.

**Recommendation:** Note in the implementation scope that these
should be consolidated into a shared utility (e.g., in
`cbsd-proto` or a shared `auth` module) during the migration.

---

### F5 — No API-layer validation of `scope_type` values (Informational)

The `ScopeBody` struct (`permissions.rs:119-124`) accepts any
string for `scope_type`. Invalid values are caught by the SQLite
`CHECK` constraint, producing a database error rather than a
clean 400 response.

When scopes move to role creation/update endpoints, this is an
opportunity to add API-layer validation of `scope_type` against
the known set (`channel`, `registry`, `repository`).

No design change needed — implementation note only.

## Prior Findings Status

All v1 findings have been addressed in the current revision:

| v1 ID | Issue | Status |
|-------|-------|--------|
| F1 | Missing fourth enforcement point | Fixed — section 4 added |
| F2 | Channel format inconsistency | Documented (lines 200–214); however, see **this review's F1** for severity upgrade |
| F3 | Flexibility trade-off not discussed | Fixed — "Trade-off" section at lines 136–143 |
| F4 | Seed builder scopes vs. empty list | Fixed — implementation note at lines 335–343 |
| F5 | Atomicity of role update | Informational — unchanged |

## Validated Claims

Factual claims checked against source code:

| # | Claim | Status |
|---|-------|--------|
| 1 | Four enforcement points exist | Confirmed |
| 2 | `require_scopes_all()` at `extractors.rs:83` | Confirmed |
| 3 | `get_user_assignments_with_scopes()` joins `user_role_scopes` | Confirmed (`db/roles.rs:329`) |
| 4 | `check_channel_scope()` at `channels/mod.rs:153` | Confirmed |
| 5 | `user_can_view_channel()` at `channels.rs:156` | Confirmed |
| 6 | `validate_descriptor_scopes` checks Channel + Repository | Confirmed (`periodic.rs:587-600`) |
| 7 | `ScopeType::Registry` never enforced | Confirmed (`#[allow(dead_code)]` on enum) |
| 8 | `SCOPE_DEPENDENT_CAPS` = `["builds:create"]` | Confirmed (`permissions.rs:47`) |
| 9 | Seed: admin `*`, builder 7 caps, viewer 2 caps, no scopes | Confirmed (`db/seed.rs:93-117`) |
| 10 | `ces-devel/*` matches both raw and composite formats | **REFUTED** — see F1 |
| 11 | `validate_scopes` requires `/` in channel patterns | Confirmed (`permissions.rs:191`) |
| 12 | Current role API has no scopes; user assignment API has scopes | Confirmed |

## Summary

The design is well-structured and the core change is sound.
The v1 findings have been addressed. The most important new
finding (F1) is a factual error about pattern matching that
understates a pre-existing inconsistency between enforcement
points 2 and 4. This should be corrected and the inconsistency
addressed — either in this design or as a prerequisite change.

**Action items before implementation:**

1. **(F1, High)** Correct the `ces-devel/*` matching claim.
   Plan how to resolve the `periodic.rs` channel format
   inconsistency (raw name vs. `channel/type`).
2. **(F2)** Document that enforcement point 3 uses different
   matching logic from the other three.
3. **(F3)** Add `user_can_view_channel()` to the
   `scopes.is_empty()` audit list.
4. **(F4)** Note `scope_pattern_matches` consolidation in
   implementation scope.
