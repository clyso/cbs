# Implementation Review: cbsd-rs Phase 10 — Periodic Builds (v2)

**Commit reviewed:**
- `76c2e27` — add periodic build scheduling (amended from `d061131`)

**Evaluated against:**
- Design: `cbsd-rs/docs/cbsd-rs/design/008-20260318T1412-periodic-builds.md` (v3)
- Prior review: `20260318T1717-impl-phase10-periodic-builds.md`

---

## Summary

Both findings from the prior review are resolved. `{DT}` now includes
seconds (`20260318T143045`), and the build submission logic is unified
in `insert_build_internal()` shared by both the REST handler and the
scheduler trigger. The duplicate `insert_build_with_periodic()` DB
function is eliminated — `insert_build()` now takes
`periodic_task_id: Option<&str>`.

**No findings. No blockers. No concerns. Implementation is clean.**

**Verdict: Approved.**

---

## Prior Findings Disposition

| Finding | Status |
|---|---|
| F1 — `{DT}` missing seconds | Resolved (`tag_format.rs:120-128`) |
| F2 — Duplicated insert/enqueue/dispatch in trigger | Resolved (`insert_build_internal()` extracted) |

---

## Verification of Fixes

### F1 — `{DT}` now includes seconds

`tag_format.rs:120-128`:
```rust
"DT" => Some(format!(
    "{:04}{:02}{:02}T{:02}{:02}{:02}",
    now.year(), now.month(), now.day(),
    now.hour(), now.minute(), now.second()
)),
```

Test updated (`tag_format.rs:277`):
```rust
assert_eq!(result, "build-20260318T143045");
```

Matches the design's specification: `{DT}` → `20260318T143020`. ✓

### F2 — Shared `insert_build_internal()`

`routes/builds.rs:169` defines:
```rust
pub async fn insert_build_internal(
    state: &AppState,
    descriptor: BuildDescriptor,
    user_email: &str,
    priority: Priority,
    periodic_task_id: Option<&str>,
) -> Result<(i64, usize), String>
```

- REST handler (`submit_build`) calls it with `periodic_task_id: None`
  at `builds.rs:125`.
- Scheduler trigger calls it with `Some(&task.id)` at
  `trigger.rs:100`.

The DB function `insert_build()` now takes `periodic_task_id: Option<&str>`
— the separate `insert_build_with_periodic()` is eliminated.

Single code path for serialize → insert → enqueue → dispatch. ✓

---

## Full Design Fidelity (re-verified)

All ~60 checkpoints from the prior review remain satisfied. The fixes
introduced no regressions — only the two changed areas were affected.
The `.sqlx/` cache (14 JSON files) is included.
