# 014 — Implementation Review: Channel/Type Mapping (v2)

**Design:**
`docs/cbsd-rs/design/014-20260323T2132-channel-type-mapping.md`
(v3)

**Plan:**
`docs/cbsd-rs/plans/014-20260324T0037-channel-type-mapping.md`

**Commits reviewed:** `bafdd77`, `675b303`
(post-plan fixup commits)

**Prior review:**
`014-20260324T0824-impl-channel-type-mapping-v1`

**Verdict:** Ready to merge

---

## v1 Finding Resolution

| # | Severity | Finding | Status |
|---|----------|---------|--------|
| F1 | Medium | `--type` not optional / `default_type_id` dead | Fixed — `version_type` now `Option<VersionType>` in proto; `resolve_and_rewrite` falls back to `channel.default_type_id`; `cbc --type` has no default |
| F2 | Medium | Trigger errors all Transient | Fixed — `classify_resolution_error` distinguishes Fatal (scope/config) from Transient (DB) by pattern matching |
| F3 | Medium | `soft_delete_type` not atomic | Fixed — both statements in a transaction |
| F4 | Medium | `user_can_view_channel` swallows errors | Fixed — returns `Result`, logs at WARN, returns 500 |
| F5 | Medium | `soft_delete_channel` no cascade | Fixed — cascades to child types in one transaction |
| F6 | Low | `set_default_type` no ownership check | Fixed — EXISTS subquery validates type belongs to channel |
| F7 | Low | Auto-set default silently discards error | Fixed — logs with `tracing::warn` |
| F8 | Low | Duplicated helpers | Addressed — cross-reference comments added |
| F9 | Low | Stale `#[allow(dead_code)]` | Fixed — removed from all now-used items |
| F10 | Low | Dead `default_marker` variable | Fixed — removed |
| F11 | Low | Inconsistent display for None | Fixed — standardized on `"-"` |

All 11 findings resolved.

---

## Review of Fixes

### F1 fix: thorough

The `version_type` change spans the full stack:

- Proto: `Option<VersionType>` with `#[serde(default)]`
- cbc: `--type` is `Option<String>`, no default value
- Resolution: falls back to `channel.default_type_id`,
  with clear error if no default is set
- Post-resolution: sets `descriptor.version_type =
  Some(resolved)` so the descriptor reaching cbscore
  always has a value (wrapper never sees None)

The cbscore wrapper is safe: `resolve_and_rewrite`
always populates `version_type` before the descriptor
is passed to the worker.

### F2 fix: pragmatic

`classify_resolution_error` pattern-matches on error
strings ("insufficient scope", "not found", "not
configured", "no default"). This is fragile — if
error messages in `resolve_and_rewrite` change, the
classification silently breaks. A typed error enum
would be more robust. However, all error messages are
internal to `resolve_and_rewrite`, so drift is
unlikely in practice. Acceptable trade-off.

### F3 + F5 fixes: clean

Both `soft_delete_type` and `soft_delete_channel` now
use transactions. The cascade in `soft_delete_channel`
uses `sqlx::query` (not `sqlx::query!`) for the
child-type update — no compile-time validation, but
the query is trivial.

### F4 fix: correct

`user_can_view_channel` returns `Result<bool, ...>`
and propagates DB errors as 500 with a WARN log.
Callers updated to use `?`.

### F6 fix: solid

The EXISTS subquery in `set_default_type` validates
the type belongs to the channel AND is active
(`deleted_at IS NULL`). The query uses `sqlx::query`
(not `sqlx::query!`) because of the subquery, but
the logic is correct.

---

## Minor Observations (not issues)

**Fallback in version_type resolution:**
`channels/mod.rs:136` has `_ => VersionType::Dev` when
converting the resolved type name back to the enum.
This is unreachable (type names are constrained by the
DB CHECK) but provides a safe default. Fine.

**Scope pattern matching in user_can_view_channel:**
The channel visibility check (routes/channels.rs:
177-188) uses inline matching logic rather than
calling `scope_pattern_matches` from channels/mod.rs.
The two checks answer different questions (channel
visibility vs. exact channel/type authorization), so
the different implementations are correct.

---

## Summary

All 11 v1 findings are resolved. The workspace compiles
cleanly. The `version_type` optional flow is correctly
wired end-to-end: the server always resolves the type
before the descriptor reaches cbscore. No new issues
found.

Ready to merge.
