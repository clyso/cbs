# Plan Review: 011 — Build Artifact Reporting

**Plan:**
`plans/011-20260321T1022-build-artifact-reporting.md`

**Design:**
`design/011-20260321T0401-build-artifact-reporting.md`
(v2, approved)

---

## Summary

The plan faithfully tracks the approved v2 design across
5 commits with correct dependency ordering. The Python
commit (Commit 2) correctly covers the full cbscore chain.
The Rust commits (3-4) split cleanly at the worker/server
boundary. One concern: the plan lists only `dispatch.rs`
for `set_build_finished` changes, but 6 call sites across
3 files need the signature update. One Python concern:
the plan's validation commands don't fully match the
`python3-cbs` skill requirements.

**Verdict: Approve with conditions.**

---

## Design Fidelity

| Design requirement | Plan |
|---|---|
| `BuildArtifactReport` Pydantic model | ✓ C2 |
| `report_version: int = 1` | ✓ (in design models) |
| `Builder.run()` returns report | ✓ C2 |
| Skipped path: minimal report | ✓ C2 |
| Report written to `/runner/scratch/` | ✓ C2 |
| `runner()` reads before `rc` check | ✓ C2 |
| `finally` cleanup of report file | ✓ C2 |
| Wrapper: `model_dump(mode="json")` | ✓ C2 |
| Compact JSON contract | ✓ C2 |
| `WrapperResult` + extraction code | ✓ C3 |
| 64 KB size limit at worker | ✓ C3 |
| `stream_output()` 3-tuple return | ✓ C3 |
| `BuildFinished` + `build_report` field | ✓ C3 |
| Migration `004_build_report.sql` | ✓ C4 |
| `BuildRecord` gains `Option<Value>` | ✓ C4 |
| `row_to_build_record` TEXT→Value | ✓ C4 |
| `list_builds` excludes report | ✓ C4 |
| `handle_build_finished` signature | ✓ C4 |
| `.sqlx/` regenerated | ✓ C4 |

---

## Commit Breakdown Assessment

### Commit 1 — docs only

Documentation checkpoint. Correct. ✓

### Commit 2 — Python (~400 lines)

Covers `report.py` (models), `builder.py` (construct +
write), `runner.py` (read + return), `wrapper.py`
(serialize + emit). These are tightly coupled — the
report models are consumed by all three. Splitting by
file would create dead-code intermediates. The 400-line
estimate is within the 400-800 target. ✓

The validation commands correctly target individual
files (per `python3-cbs` skill: "always target only the
files being actively edited").

**Concern C1:** The plan validates with `uv run ruff`,
`uv run basedpyright` — correct. But it runs `ruff
format` before `ruff check`, which is the right order
per the skill. However, it should also run `ruff check
--fix` (not just `ruff check`) per the pre-commit
checklist. Minor — the lint step can auto-fix.

**Concern C2:** `Builder.run()` changes from
`async def run(self) -> None` to
`async def run(self) -> BuildArtifactReport | None`.
The plan says "returns `BuildArtifactReport | None`"
which is correct modern Python syntax. The type hint
uses union syntax, not `Optional`. ✓

### Commit 3 — Rust worker (~200 lines)

`cbsd-proto/src/ws.rs` + `cbsd-worker/src/build/output.rs`
+ `cbsd-worker/src/ws/handler.rs`. The proto change and
the worker extraction are tightly coupled (the handler
constructs the `BuildFinished` message using the output
from `stream_output`). 200 lines is below the guideline
but the commit is meaningful and independently testable
(worker compiles and forwards reports; server doesn't
store them yet). ✓

### Commit 4 — Rust server (~300 lines)

Migration + DB + dispatch + routes + `.sqlx/`. All
tightly coupled — the migration must land with the
query changes. Within the 400-800 range. ✓

### Commit 5 — docs only

Implementation review documents. ✓

---

## Concerns

### C1 — `set_build_finished` has 6 call sites across 3 files

The plan lists only `dispatch.rs` for the
`set_build_finished` signature change. The actual call
sites are:

| File | Line | Context |
|---|---|---|
| `dispatch.rs:342` | `handle_build_finished` | ← report here |
| `dispatch.rs:407` | `handle_build_rejected` | failure, `None` |
| `dispatch.rs:565` | ack timeout | revoke, `None` |
| `handler.rs:787` | `handle_worker_dead` (revoking) | `None` |
| `handler.rs:822` | `fail_build` | `None` |
| `main.rs:442` | drain/revoke | `None` |

When `set_build_finished` gains a new `build_report`
parameter, ALL 6 call sites must be updated (passing
`None` for the non-success paths). The plan only
mentions `dispatch.rs`. The `handler.rs` and `main.rs`
call sites are missing.

**Fix:** Add `handler.rs` and `main.rs` to Commit 4's
file list with a note that their `set_build_finished`
calls pass `None` for the report.

### C2 — `cbscore-wrapper.py` is in `cbsd-rs/scripts/` but validated as Python

The plan's Commit 2 includes `cbsd-rs/scripts/
cbscore-wrapper.py` in the Python validation commands.
This file is part of the `cbsd-rs` workspace, not the
`cbscore` package. The `basedpyright` invocation may
not find the correct `pyrightconfig.json` for this file.
Verify that basedpyright can type-check the wrapper in
isolation (it imports from both `cbscore` and `cbsd-rs`
packages).

### C3 — `runner()` return type annotation change

The current signature is `async def runner(...) -> None`.
The plan says it changes to return
`BuildArtifactReport | None`. This is a breaking change
to the public API of `cbscore.runner` — any callers of
`runner()` that don't expect a return value will
silently ignore it (Python doesn't enforce return type
consumption). This is fine for the wrapper (which will
capture it), but other callers (if any) should be
checked.

---

## Minor Notes

- **Dependency ordering is correct.** Commit 2 (Python)
  → Commit 3 (worker) → Commit 4 (server). The Rust
  side handles `None`/missing gracefully, so commits 3-4
  compile independently of commit 2.

- **The `model_dump(mode="json")` call** is correct
  Pydantic v2 idiom. Returns a plain dict suitable for
  `json.dumps`.

- **`report.py` naming.** The plan places Pydantic
  models in `cbscore/src/cbscore/builder/report.py`.
  This follows cbscore's existing pattern of having
  model files alongside the code that constructs them.

- **The `finally` block for report cleanup** is in
  `runner.py`, not `builder.py`. Correct — `runner()`
  reads the file from the host side; it owns cleanup.

---

## No Blockers Found

The plan is a faithful translation of the approved
design into 5 commits with correct ordering. The main
concern (C1: missing call sites) is a file-list
omission, not a design or logic error — the
implementation will naturally discover the compiler
errors from the signature change and fix all sites.
