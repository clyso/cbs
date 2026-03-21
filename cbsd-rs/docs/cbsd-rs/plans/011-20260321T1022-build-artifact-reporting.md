# 011 — Build Artifact Reporting: Implementation Plan

**Design:**
`docs/cbsd-rs/design/011-20260321T0401-build-artifact-reporting.md`
(v2, approved)

## Commit Breakdown

5 commits across Python and Rust, ordered by dependency.

---

### Commit 1: `cbsd-rs/docs: add build artifact reporting design and plan`

**Documentation only**

Add design document (v2), implementation plan, and all
design/plan reviews.

**Files:**

| File | Change |
|------|--------|
| `docs/cbsd-rs/design/011-20260321T0401-build-artifact-reporting.md` | Design (v2, approved) |
| `docs/cbsd-rs/plans/011-20260321T1022-build-artifact-reporting.md` | This plan |
| `docs/cbsd-rs/reviews/011-*` | All design and plan reviews |

---

### Commit 2: `cbscore: add artifact report and return from builder/runner`

**~400 authored lines (Python)**

New Pydantic models for the build artifact report, wired
end-to-end through the Python stack: `Builder.run()`
constructs the report and writes it to the scratch volume,
`runner()` reads it from the host side, and
`cbscore-wrapper.py` includes it in the result JSON line.

**Files:**

| File | Change |
|------|--------|
| `cbscore/src/cbscore/builder/report.py` | New: `BuildArtifactReport`, `ContainerImageReport`, `ReleaseDescriptorReport`, `ComponentReport` Pydantic models |
| `cbscore/src/cbscore/builder/builder.py` | `run()` returns `BuildArtifactReport \| None`; constructs report from `ReleaseDesc` and container image info; writes to `/runner/scratch/build-report.json`; handles `skipped` case with minimal report |
| `cbscore/src/cbscore/runner.py` | Return type → `BuildArtifactReport \| None`; read report from `config.paths.scratch / "build-report.json"` before `rc != 0` check; `finally` block deletes stale file |
| `cbsd-rs/scripts/cbscore-wrapper.py` | Include `build_report` in result JSON line via `model_dump(mode="json")` |

**Key details:**

- `builder.py`: The `skipped` early return (line ~114)
  produces a minimal report with `skipped=True`,
  `container_image` populated, `components=[]`,
  `release_descriptor=None`.
- `runner.py`: Report file read is **before** `if rc != 0`
  so partial reports are captured. Cleanup is in `finally`.
- `wrapper.py`: Compact JSON with
  `separators=(",",":")` preserved.
- `runner()` return type changes from `None` to
  `BuildArtifactReport | None`. This is safe: Python
  does not enforce return-value consumption, so existing
  callers (if any beyond the wrapper) silently ignore the
  new return. Verify no other callers exist via
  `grep -r "runner(" cbscore/`.

**Validation:**

```bash
uv run ruff format \
    cbscore/src/cbscore/builder/report.py \
    cbscore/src/cbscore/builder/builder.py \
    cbscore/src/cbscore/runner.py \
    cbsd-rs/scripts/cbscore-wrapper.py
uv run ruff check --fix \
    cbscore/src/cbscore/builder/report.py \
    cbscore/src/cbscore/builder/builder.py \
    cbscore/src/cbscore/runner.py \
    cbsd-rs/scripts/cbscore-wrapper.py
uv run basedpyright \
    cbscore/src/cbscore/builder/report.py \
    cbscore/src/cbscore/builder/builder.py \
    cbscore/src/cbscore/runner.py

# Note: cbscore-wrapper.py lives in cbsd-rs/scripts/, outside
# the cbscore package. basedpyright may not resolve its imports
# correctly without the cbscore virtualenv. Verify manually that
# the wrapper type-checks, or run basedpyright from the repo
# root where both packages are visible.
```

---

### Commit 3: `cbsd-rs: add build_report to WebSocket protocol and worker`

**~200 authored lines (Rust)**

Extend `BuildFinished` in `cbsd-proto` with an optional
`build_report` field. Update the worker's `output.rs` to
extract the report from the wrapper result line (with
64 KB size limit) and forward it in the `BuildFinished`
WebSocket message.

**Files:**

| File | Change |
|------|--------|
| `cbsd-proto/src/ws.rs` | Add `build_report: Option<serde_json::Value>` to `BuildFinished` variant |
| `cbsd-worker/src/build/output.rs` | Extend `WrapperResult` with `build_report`; extract via `parsed.get("build_report").cloned()`; enforce 64 KB limit; change `stream_output()` return to 3-tuple |
| `cbsd-worker/src/ws/handler.rs` | Pass `build_report` through to `BuildFinished` message construction (all call sites) |

**Validation:**

```bash
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

---

### Commit 4: `cbsd-rs/server: store and expose build artifact report`

**~300 authored lines (Rust)**

Add the `build_report` column to the database, store it on
`build_finished`, and expose it in the single-build API
response. The list endpoint excludes the report.

**Files:**

| File | Change |
|------|--------|
| `migrations/004_build_report.sql` | New: `ALTER TABLE builds ADD COLUMN build_report TEXT` |
| `cbsd-server/src/db/builds.rs` | `BuildRecord` gains `build_report: Option<serde_json::Value>`; `row_to_build_record` deserializes TEXT→Value; `get_build` query updated to SELECT new column; `list_builds` query does NOT select `build_report`; new `set_build_report()` function |
| `cbsd-server/src/ws/dispatch.rs` | `handle_build_finished()` and `set_build_finished()` gain `build_report` parameter; `handle_build_rejected()` and ack-timeout paths pass `None` |
| `cbsd-server/src/ws/handler.rs` | `handle_worker_dead()` and `fail_build()` call sites pass `None` for report |
| `cbsd-server/src/main.rs` | Drain/revoke `set_build_finished` call passes `None` |
| `cbsd-server/src/routes/builds.rs` | `get_build` response includes `build_report`; list response excludes it (separate response type or `#[serde(skip)]` on list records) |
| `.sqlx/` | Regenerated after migration + query changes |

**Note:** `set_build_finished` has 6 call sites across
`dispatch.rs`, `handler.rs`, and `main.rs`. Only the
success path in `handle_build_finished` passes the actual
report; all other call sites (rejection, timeout, dead
worker, drain) pass `None`.

**Validation:**

```bash
# Regenerate sqlx cache
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx database create
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx migrate run
DATABASE_URL=sqlite:///tmp/cbsd-dev.db \
    cargo sqlx prepare --workspace

# Build and test
SQLX_OFFLINE=true cargo build --workspace
cargo test --workspace
```

---

### Commit 5: `cbsd-rs/docs: add implementation reviews for build artifact reporting`

**Documentation only**

Post-implementation review documents.

**Files:**

| File | Change |
|------|--------|
| `docs/cbsd-rs/reviews/011-*-impl-*` | Implementation review(s) |

---

## Dependency Graph

```
Commit 1 (docs: design, plan, reviews)
    ↓
Commit 2 (Python: models + builder + runner + wrapper)
    ↓
Commit 3 (Rust: proto + worker)
    ↓
Commit 4 (Rust: server storage + API)
    ↓
Commit 5 (docs: implementation reviews)
```

Commits 3-4 depend on commit 2 (wrapper must emit
`build_report` before the worker can parse it), but the
Rust side handles `None`/missing gracefully, so they
compile independently.

## Progress

| # | Commit | Status |
|---|--------|--------|
| 1 | docs: design, plan, reviews | Done |
| 2 | cbscore: models + builder + runner + wrapper | Done |
| 3 | cbsd-rs: proto + worker report forwarding | Done |
| 4 | cbsd-rs/server: storage + API | Done |
| 5 | docs: implementation reviews | Done |
