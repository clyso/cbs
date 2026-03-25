# 013 — Implementation Review: Extended Version Info (v2)

**Design:**
`docs/cbsd-rs/design/013-20260322T1210-extended-version-info.md`
(v2)

**Plan:**
`docs/cbsd-rs/plans/013-20260322T1504-extended-version-info.md`

**Commits reviewed:** `aff6855..c9e08b0` (plan commits 2, 3, 4)

**Prior review:** `013-20260322T1603-impl-extended-version-info-v1`

**Verdict:** Ready to merge

---

## v1 Finding Resolution

| # | Severity | Finding | Status |
|---|----------|---------|--------|
| F1 | Critical | `//foo` in `cbc/src/main.rs:1` | Fixed — removed in `aff6855` |
| F2 | Important | `skip_serializing_if` on workers API version | Fixed — annotation removed |
| F3 | Important | Missing `.gitignore` for `.git-version` | Fixed — added in `aff6855` |
| F4 | Minor | Test lacks version assertion | Fixed — both tests now assert version |
| F5 | Minor | Silent acceptance of versionless workers | Fixed — `match` with DEBUG log for `None` |
| F6 | Observation | Version lost on disconnect | Acknowledged — by-design |

All findings from v1 are resolved. No new issues found.

---

## Review of Fixes

### F1 fix: clean

`//foo` removed from `cbc/src/main.rs`. The committed
file now starts with the copyright header as expected.

### F2 fix: clean

`WorkerInfoResponse.version` is now a bare
`Option<String>`. Offline workers will serialize as
`"version": null`, matching the design. No other
annotations were disturbed.

### F3 fix: well-placed

`.git-version` added to `cbsd-rs/.gitignore` in commit
`aff6855` — the same commit that introduces the `build.rs`
files reading it. Correct commit boundary.

### F4 fix: thorough

Round-trip test (`ws.rs:218-220`) now destructures
`version` and asserts:


```rust
assert_eq!(version.as_deref(), Some("0.1.0+gtest123"));
```

Backwards-compat test (`ws.rs:228-233`) adds an explicit

comment and assertion:

```rust
// No version field in JSON — tests backwards compat
// via serde(default).
assert_eq!(version, None);
```

Both arms of the contract (present and absent) are now
tested.

### F5 fix: idiomatic

The `if let Some` was replaced with a three-arm `match`
(`ws/handler.rs:240-256`):

```rust
match worker_version {
    Some(ref wv) if wv != crate::VERSION => { /* WARN */ }
    None => { /* DEBUG */ }
    _ => {}
}
```

Clean, exhaustive, and the `DEBUG` level is appropriate —
it won't clutter production logs but is visible when
troubleshooting.

---

## Plan Correlation

| Plan | SHA | Subject | Files | LOC |
|------|-----|---------|-------|-----|
| C2 | `aff6855` | embed git version in all binaries | 8 | +105/-4 |
| C3 | `8b4c628` | report worker version in WS Hello | 6 | +63/-18 |
| C4 | `c9e08b0` | prod build script, drop compose prod | 4 | +129/-67 |

All three commits map 1:1 to the plan. File lists match
exactly. Dependency ordering is correct. Each commit
compiles independently. Working tree is clean.

Commit 2 includes `.gitignore` (F3 fix). Commit 3 has
improved test assertions and the version skew match
rewrite (F4/F5 fixes). Commit 4 is unchanged from v1.

---

## Summary

| Severity | Count | Action |
|----------|-------|--------|
| Fixed | 5 | F1, F2, F3, F4, F5 all verified |
| Observation | 1 | F6 acknowledged (by-design) |

All v1 findings resolved. Ready to merge.
