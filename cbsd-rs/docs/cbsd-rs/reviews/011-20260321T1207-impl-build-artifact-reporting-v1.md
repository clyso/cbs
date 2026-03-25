# Implementation Review: 011 — Build Artifact Reporting

**Commits reviewed:**


- `02a72b1` — docs: design, plan, reviews
- `2a3a06f` — cbscore: artifact report + builder/runner
- `0e5eb6b` — cbsd-rs: WebSocket protocol + worker
- `56a3895` — cbsd-rs/server: storage + API


**Evaluated against:**

- Design `011-20260321T0401` (v2, approved)
- Plan `011-20260321T1022` (v2, approved)

---

## Summary

The implementation faithfully tracks the approved design
across all 4 commits. The Python models are clean Pydantic
v2, the Rust changes are minimal and correct, the 64 KB
size limit is enforced, and the backwards-compatibility
story works at every layer. The `BuildListRecord` split
for the list endpoint is a clean solution.

One finding: the `_emit_result` function always emits
`"build_report": null` in the JSON (even for error paths),
which adds ~20 bytes to every result line. The Rust side
handles this correctly (filters null to None). One minor
observation about a `let _ = build_report` placeholder in
Commit 3 that was correctly removed in Commit 4.

**Verdict: Approved. No findings require changes.**

---

## Design Fidelity

| Requirement | Status |
|---|---|
| `BuildArtifactReport` Pydantic model | ✓ `report.py` |
| `report_version: int = 1` | ✓ |
| `ContainerImageReport`, `ReleaseDescriptorReport`, `ComponentReport` | ✓ |
| `Builder.run()` → `BuildArtifactReport \| None` | ✓ |
| Skipped path: minimal report | ✓ |
| Report written to scratch path | ✓ (`self.scratch_path / "build-report.json"`) |
| `runner()` reads before `rc` check | ✓ |
| `finally` cleanup of report file | ✓ |
| Wrapper: `model_dump(mode="json")` | ✓ |
| Compact JSON preserved | ✓ |
| `WrapperResult` + extraction code | ✓ |
| `parsed.get("build_report").cloned()` | ✓ |
| 64 KB size limit | ✓ (`MAX_REPORT_SIZE`) |
| JSON null filtered to None | ✓ |
| `BuildFinished` + `build_report` field | ✓ |
| `serde(default, skip_serializing_if)` | ✓ |
| Proto tests for round-trip + missing field | ✓ (2 new tests) |
| Migration `004_build_report.sql` | ✓ |
| `BuildRecord` gains `Option<Value>` | ✓ |
| TEXT→Value deserialization in `get_build` | ✓ |
| `BuildListRecord` separate type (no report) | ✓ |
| `list_builds` SQL unchanged (no `build_report`) | ✓ |
| `set_build_finished` gains `build_report` param | ✓ |
| All 6 call sites updated | ✓ |
| `.sqlx/` cache regenerated | ✓ |
| Only success path stores report | ✓ |
| `handle_build_finished` report param | ✓ |
| `cbc` client unaffected | ✓ (ignores unknown field) |

---

## Commit-by-Commit Verification

### 02a72b1 — docs

5 files + 1 new directory. Design v2, plan, reviews.
All match approved documents. ✓

### 2a3a06f — Python (~220 lines)


**`report.py`:**

- Clean Pydantic v2 models with docstrings.
- All fields typed. `str | None` union syntax. ✓
- Logger follows cbscore pattern:
  `parent_logger.getChild("report")`. ✓

- `report_version: int = 1` for schema evolution. ✓

**`builder.py`:**

- `async def run(self) -> BuildArtifactReport | None`
  — return type annotation correct. ✓
- Skipped path: minimal report with `skipped=True`,
  `container_image` populated, `components=[]`,
  `release_descriptor=None`. Writes to scratch. ✓
- Full build path: `_build_report()` extracts data from
  `ReleaseDesc.builds.values()` → components. ✓
- `_write_report()` catches `OSError`, logs warning on
  failure (non-fatal). ✓

- f-string logging replaced with `%s`-style in the
  `skopeo_image_exists` message. ✓

**`runner.py`:**

- Return type → `BuildArtifactReport | None`. ✓
- `report_host_path` set before `try` block. ✓
- Read + validate + cleanup in `try/finally` after the
  `podman_run` `finally` (components cleanup). ✓
- `model_validate_json(raw)` — correct Pydantic v2 API
  for JSON string validation. ✓
- `report_host_path.unlink(missing_ok=True)` in `finally`. ✓
- Report read before `if rc != 0` check. ✓

- `return report` after the success path. ✓
- Broad `except Exception` for report read is acceptable:
  a corrupt report file should not crash the runner. ✓

**`cbscore-wrapper.py`:**

- `_emit_result` gains `build_report: dict[str, object] | None = None`. ✓
- `report.model_dump(mode="json")` — correct Pydantic v2
  idiom for JSON-safe dict. ✓
- Error paths call `_emit_result(1/2, ...)` without
  `build_report` — defaults to `None`. ✓

- Result dict always includes `"build_report": null` for
  error paths. The Rust side handles this. ✓

### 0e5eb6b — Rust worker (~88 lines)

**`ws.rs` (proto):**


- `build_report: Option<serde_json::Value>` with
  `serde(default, skip_serializing_if)`. ✓
- 2 new tests: round-trip with report, missing field
  defaults to None. ✓
- Existing tests updated to include `build_report: None`. ✓

**`output.rs`:**


- `WrapperResult.build_report: Option<Value>`. ✓
- Extraction: `parsed.get("build_report").cloned()`. ✓
- Size limit: `MAX_REPORT_SIZE = 65_536`. ✓

- Null filtering: `is_some_and(Value::is_null)`. ✓
- Return type: 3-tuple. ✓

**`cbsd-worker/ws/handler.rs`:**

- All `BuildFinished` construction sites updated. ✓
- Error paths pass `build_report: None`. ✓


**`cbsd-server/ws/handler.rs` (in this commit):**

- `let _ = build_report;` placeholder — correctly

  suppresses unused-variable warning until Commit 4
  wires it up. ✓

### 56a3895 — Rust server (~99 lines)

**`004_build_report.sql`:**

- `ALTER TABLE builds ADD COLUMN build_report TEXT`. ✓
- Comment documents NULL semantics. ✓

**`builds.rs`:**


- `BuildRecord` gains `build_report: Option<Value>`
  with `skip_serializing_if`. ✓
- `BuildListRecord` — separate type without report. ✓
- `get_build` query SELECTs `build_report`, deserializes

  TEXT→Value via `serde_json::from_str`. ✓
- `list_builds` returns `Vec<BuildListRecord>`. ✓
- `row_to_build_list_record` — renamed from
  `row_to_build_record`. ✓
- `set_build_finished` gains `build_report: Option<&str>`. ✓

**`dispatch.rs`:**


- `handle_build_finished` gains `build_report` param. ✓
- `handle_build_rejected` passes `None`. ✓
- `handle_revoke_timeout` passes `None`. ✓


**`handler.rs`:**


- `let _ = build_report` removed. ✓
- Success path: serializes Value→String, passes to
  `handle_build_finished`. ✓
- Non-success: passes `None`. ✓
- `handle_worker_dead`: `set_build_finished` with `None`. ✓
- `fail_build`: `set_build_finished` with `None`. ✓

**`main.rs`:**

- Drain/revoke: `set_build_finished` with `None`. ✓

**`routes/builds.rs`:**

- `list_builds` returns `Vec<BuildListRecord>`. ✓

**`.sqlx/`:**

- 2 updated cache files. ✓

---

## Observations

- **`_emit_result` always emits `"build_report": null`.**
  The result dict is `{"type":"result", "exit_code":N,
  "error":..., "build_report":null}`. This adds ~20 bytes
  to every result line, including error paths. The Rust
  side's null-to-None filter handles this correctly. An
  alternative would be to omit the key when `None` (like
  `skip_serializing_if` in serde), but Python's
  `json.dumps` doesn't have a built-in equivalent for
  `None` values. Acceptable as-is — the overhead is
  negligible.

- **`_build_report` iterates `release_desc.builds.values()`**
  which may contain multiple build entries (one per
  architecture or distro variant). Each entry's components
  are appended to the flat `components` list. This is
  correct — a multi-arch build will report all components
  from all build entries.

- **No `TODO` markers in the implementation.** The
  Commit 3 `let _ = build_report` was a cross-commit
  placeholder, not a TODO — and it's resolved in Commit 4.
  No deferred work remains.

- **No dead code.** `BuildListRecord` is used by
  `list_builds`. `BuildRecord.build_report` is used by
  `get_build`. The `_write_report` helper is called from
  both the skipped and success paths.

- **Future expansion.** The `report_version: int = 1`
  field enables schema evolution. The `serde_json::Value`
  approach in the worker and server means adding new
  Python-side fields requires no Rust changes. The
  `BuildListRecord` / `BuildRecord` split is extensible.

- **Partial reports on failure are deferred.** When
  `runner()` raises `RunnerError` (rc != 0), the report
  is read from the file but the exception propagates to
  the wrapper, which catches `RunnerError` and emits
  `exit_code: 1` without the report. The report object
  is lost in the exception path. The design documented
  this as a future iteration — the report would need to
  be attached to the exception or returned alongside it.
  Not a gap in this implementation.
