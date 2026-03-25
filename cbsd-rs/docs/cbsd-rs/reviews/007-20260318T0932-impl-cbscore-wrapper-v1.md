# Implementation Review: cbsd-rs Phase 9 — cbscore Wrapper

**Commit reviewed:**


- `ab96acc` — implement cbscore-wrapper.py build bridge (180+, 19−, 2 files)


**Evaluated against:**

- Design: `cbsd-rs/docs/cbsd-rs/design/007-20260318T0716-cbscore-wrapper.md`

---

## Summary

Phase 9 is a clean, well-structured commit that delivers the working
wrapper and the executor env var changes as a single atomic unit. The
wrapper faithfully follows the design's 12-step procedure, including the
complete `version_create_helper()` call with all 12 parameters, the
stderr redirect, the error classification with `[infra]` prefixes, and
the temp file cleanup. The Rust-side changes are minimal and correct.

One finding: `version_create_helper()` exceptions (`VersionError`,
`MalformedVersionError`) are not caught by the wrapper's error handling.
Only `ImportError` is caught around that call. These exceptions produce
an unstructured traceback instead of the structured result line, causing
the Rust worker to classify the build as `Failure` with no error message.

**Verdict: One finding to fix. Otherwise approved.**

---

## Design Fidelity

| Design requirement | Status |
|---|---|
| `os.dup2(1, 2)` as first operation (step 1) | ✓ (line 36, before imports) |
| Read JSON from stdin (step 2) | ✓ (lines 60-64) |
| Load config from `CBSCORE_CONFIG` env var (step 3) | ✓ (lines 80-94) |
| Validate `config.storage.registry` (step 3) | ✓ (lines 96-97) |
| Override `config.paths.components` (step 4) | ✓ (line 100) |
| Parse `os_version` → `el_version` with regex (step 5) | ✓ (lines 46-51) |
| `version_create_helper()` with all 12 params (step 6) | ✓ (lines 109-129) |
| `component_refs` uses `c["ref"]` (not `"git_ref"`) | ✓ (line 113) |
| `component_uri_overrides` uses dict with `c.get("repo")` guard | ✓ (lines 117-121) |
| Temp file via `mkstemp(prefix="cbsd-wrapper-")` (step 7) | ✓ (line 134) |
| Close fd after writing (step 7) | ✓ (`os.fdopen(fd, "w")` + context manager) |
| `Path.unlink(missing_ok=True)` in `finally` (step 12) | ✓ (line 187) |
| Resolve `cbscore_path` from env or import (step 8) | ✓ (lines 141-147) |
| Validate `_tools/cbscore-entrypoint.sh` (step 8) | ✓ (lines 149-154) |
| `asyncio.run(runner(...))` (step 9) | ✓ (line 170) |
| `run_name` uses 12 hex chars from stripped UUID | ✓ (line 158) |
| `replace_run=True` | ✓ (line 176) |
| `timeout` from `CBS_BUILD_TIMEOUT` or 7200 | ✓ (line 157) |
| `log_out_cb` async with `end=""` and `flush=True` | ✓ (lines 160-163) |
| Compact JSON result via `separators=(",", ":")` | ✓ (line 42) |
| `[infra]` prefix on exit-2 errors | ✓ (all exit-2 paths) |
| `RunnerError`, `VersionError`, `MalformedVersionError` → exit 1 | ✓ (line 182) |
| Startup log line | ✓ (lines 69-72) |
| `CBS_TRACE_ID` not re-set by wrapper | ✓ (line 57 reads env, no os.environ set) |
| `cbsdcore` NOT a dependency | ✓ (no cbsdcore import) |
| Validate `dst_image.tag` non-empty | ✓ (lines 75-77) |
| **Executor:** `CBSCORE_CONFIG` env var | ✓ (executor.rs:144) |
| **Executor:** `CBS_BUILD_TIMEOUT` conditional | ✓ (executor.rs:150-152) |
| **Executor:** `MissingConfig` guard | ✓ (executor.rs:120-124) |
| **Executor:** `stderr(Stdio::null())` | ✓ (executor.rs:147) |
| Single atomic commit (wrapper + executor) | ✓ |

---

## Findings

### F1 — `version_create_helper()` exceptions escape unhandled

The `try/except` around `version_create_helper()` (lines 106-131) only
catches `ImportError`:

```python
try:
    from cbscore.versions.create import version_create_helper
    version_desc = version_create_helper(...)
except ImportError as e:
    _emit_result(2, f"[infra] cbscore not installed: {e}")
```

If `version_create_helper()` raises `VersionError`,
`MalformedVersionError`, or any other exception (e.g., `KeyError` from
a malformed descriptor, `TypeError` from unexpected field types), the
exception propagates out of `main()` unhandled.

Python will print the traceback to stdout (since stderr is redirected)
and exit with code 1. The Rust worker sees no structured result line
(the traceback doesn't start with `{"type":"result"`), so
`stream_output` returns `(Failure, None)` — correct outcome but with
no error message.


The design's error classification table maps:

- `VersionError` → exit 1
- `MalformedVersionError` → exit 1

These are currently only caught in the inner `try/except` at lines
169-185 which wraps `runner()`, not `version_create_helper()`.

**Impact:** A malformed version string or missing component produces a
Python traceback in the build log instead of a clean error message. The
build is correctly marked as failed but the error field is empty.

**Fix:** Either (a) widen the `except` block at line 130 to catch the
build exceptions:

```python
except ImportError as e:
    _emit_result(2, f"[infra] cbscore not installed: {e}")
except (VersionError, MalformedVersionError) as e:
    _emit_result(1, str(e))
except Exception as e:
    _emit_result(2, f"[infra] version_create_helper failed: {e}")
```

Or (b) move `version_create_helper()` inside the outer `try:` block at
line 136 and add the exception types to the inner `try/except`. Option
(a) is simpler and keeps the current structure.

Note: this requires importing `VersionError` and `MalformedVersionError`
at the top of the `except` block or moving the imports earlier. The
current imports (lines 165-167) are inside the `try:` at line 136, after
the `version_create_helper()` call.

Severity: **Medium.** The build still fails correctly (exit 1 from
Python's unhandled exception path). The user just gets a traceback
instead of a clean error message.

---

## Observations

- **`_emit_result` uses `sys.exit()` as a control flow mechanism.**
  All error paths call `_emit_result` which raises `SystemExit`
  (a `BaseException`, not caught by `except Exception`). This pattern
  works correctly throughout — the `finally` block at line 186 runs on
  `SystemExit`, cleaning up the temp file. The calls before line 136
  (where no temp file exists yet) also work because there's nothing to
  clean up. Sound pattern.

- **`os.fdopen(fd, "w")` correctly transfers fd ownership.** The
  `with` block at line 137 closes the fd on exit. No fd leak. The
  temp file path is then passed to `runner()` which opens it separately.
  Correct.

- **`trace_id` fallback chain is sensible.** Line 57 reads from env
  (set by executor), line 64 overrides from stdin if present. Both
  sources should agree. If neither is present, "unknown" is used.
  Not a concern.

- **`config_path_str` fallback to `"cbs-build.config.yaml"`.** Design
  says the cwd fallback is "for manual testing only." The implementation
  correctly does not log or warn about the fallback — it just tries it.
  If it fails, `ConfigError` is caught at line 91. Acceptable.

- **Import organization.** cbscore imports are deferred (inside
  functions, after `os.dup2`). This is intentional — it ensures stderr
  is redirected before any cbscore module-level logging fires. The
  imports at lines 86, 107, 145, 165-167 are all after line 36. Correct.

- **`model_dump_json()` at line 138.** This is the Pydantic v2 method
  for serializing to JSON. It confirms that `version_create_helper()`
  returns a Pydantic `VersionDescriptor` model. Correct usage.

---

## Rust-Side Verification

The executor changes are minimal and correct:

- `ExecutorError::MissingConfig(&'static str)` — new variant, properly
  handled in `Display` and `Error::source()` impls.
- Guard at lines 120-124 extracts `cbscore_config_path` or returns
  `MissingConfig`. Fires before any subprocess work.
- `.env("CBSCORE_CONFIG", cbscore_config_path)` at line 144.
- `.stderr(Stdio::null())` at line 147.
- Optional `CBS_BUILD_TIMEOUT` at lines 150-152.
- `CBSCORE_PATH` intentionally not set (wrapper derives it from import).

All match the design and plan.

---

## Commit Quality

- **~200 LOC Python + ~20 LOC Rust** — matches the plan's estimate.
- **Atomic:** wrapper + executor changes in one commit. Neither works
  without the other.
- **No logic changes to existing code** beyond the executor env var
  additions and stderr change.
- **Clear commit message** referencing the design document.
