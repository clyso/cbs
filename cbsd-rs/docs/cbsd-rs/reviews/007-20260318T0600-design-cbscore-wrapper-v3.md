# Design Review: cbscore Wrapper v3 — Python Subprocess Bridge

**Document reviewed:**
- `cbsd-rs/docs/cbsd-rs/design/007-20260318T0716-cbscore-wrapper.md` (v3, third revision)

**Cross-referenced against:**
- `cbsd-rs/cbsd-worker/src/build/executor.rs`
- `cbsd-rs/cbsd-worker/src/build/output.rs`
- `cbsd-rs/cbsd-worker/src/config.rs`
- `cbsd-rs/cbsd-proto/src/build.rs`
- `cbscore/src/cbscore/runner.py`
- `cbscore/src/cbscore/versions/create.py`
- `cbsd/cbslib/worker/builder.py`

---

## Summary

The design is well-matured after three revision passes. All 4 v1 blockers
and both v2 blockers are fully resolved in the document. The complete
12-parameter `version_create_helper()` call, the `MalformedVersionError`
classification, the stderr redirect rationale, the async `runner()`
invocation, the component path override, and the SIGTERM limitation are
all correctly specified.

One blocker remains: `executor.rs` still only passes `CBS_TRACE_ID` to
the subprocess. The design correctly specifies the required `.env()` calls
and the `Stdio::null()` change, but the code has not been updated. This
is not a design deficiency — it is a pending implementation prerequisite
that must ship alongside the wrapper.

No design-level blockers. The design is approved for implementation
with the conditions listed below.

**Verdict: Approve with conditions.**

---

## Prior Blocker Disposition

| Blocker | Status |
|---|---|
| v1 B1 — `CBSCORE_CONFIG` not passed | Design resolved (code pending) |
| v1 B2 — async `runner()` | Resolved (step 9) |
| v1 B3 — `component_path` doesn't reach `runner()` | Resolved (step 4) |
| v1 B4 — `cbscore_path` under-specified | Resolved (step 8) |
| v2 B1 — incomplete `version_create_helper()` params | Resolved (step 6, all 12 params) |
| v2 B2 — `MalformedVersionError` missing | Resolved (error table) |

---

## Blockers

None at the design level. The executor env var gap is an implementation
prerequisite, not a design deficiency.

---

## Major Concerns

### M1 — `executor.rs` must be updated before the wrapper is functional

The design correctly specifies that `executor.rs` must pass
`CBSCORE_CONFIG`, `CBSCORE_PATH`, `CBS_BUILD_TIMEOUT` and change stderr
to `Stdio::null()`. The code has not been updated. Without at minimum
`CBSCORE_CONFIG`, every real build exits 2 with no useful output.

**Condition:** The executor env var change and the wrapper implementation
must ship in the same commit or be sequenced (executor first). The
design should add a "Prerequisite Rust change" subsection making the
dependency explicit. When `cbscore_config_path` is `None`, the executor
should return `ExecutorError` at spawn time rather than letting the
subprocess fail.

### M2 — `image_tag` empty-string edge case

`BuildDestImage.tag` is `String` (non-optional) in the Rust proto.
`version_create_helper()` accepts `str | None`. An empty string `""`
is falsy in Python, triggering the fallback
`image_tag_str = image_tag if image_tag else version` at
`create.py:133` — silently substituting the version string as the tag.
No error is raised.

**Condition:** Add a wrapper check that `dst_image.tag` is non-empty
(exit 2 if empty), or document the fallback behavior explicitly. This
should be addressed before production deployment.

### M3 — Exit code 1 vs 2 is lost at the `BuildFinishedStatus` level

`classify_exit_code()` in `executor.rs` maps both exit 1 and exit 2
to `BuildFinishedStatus::Failure`. The design defines a meaningful
distinction (build failure vs infrastructure failure) that is not
observable by the server or operators.

**Condition:** At minimum, propagate the exit code in the error message
field (e.g., `"[exit 2] entrypoint not found"`) so operators can
distinguish infrastructure failures from build failures in logs. A
structured `InfraFailure` status is a larger change for a future phase.

---

## Minor Issues

- **`log_out_cb` double-newline.** `runner.py:97-99` normalizes lines
  to end with `\n`. The wrapper's `print(msg, flush=True)` adds another
  newline. Use `print(msg, end="", flush=True)` to avoid double-spaced
  build logs.

- **`tempfile.mkstemp()` fd handling.** Step 7 says "explicit close
  before passing to `runner()`." Clarify that this refers to the fd
  returned by `mkstemp()` (not the file object after writing). The fd
  must be closed after writing the JSON, before `runner()` opens the
  path separately.

- **`run_name` collision space.** 8 hex chars = 2^32 values. With
  `replace_run=True`, a collision between two concurrent builds kills
  the other's container. Using 12-16 chars from the stripped UUID would
  reduce risk to effectively zero with no operational cost.

- **Python interpreter path.** `executor.rs` hardcodes
  `Command::new("python3")`. In containers with `uv`-managed venvs,
  the `python3` on `PATH` may not be the interpreter where cbscore is
  installed. The design should note this requirement or make the Python
  path configurable.

- **`NoSuchVersionDescriptorError` not in error table.** Raised by
  `runner.py:196` if the temp file doesn't exist. Falls through to the
  catchall (exit 2) — correct disposition but worth documenting for
  completeness.

- **`tempfile.mkstemp` prefix.** Adding `prefix="cbsd-wrapper-"` makes
  leaked temp files identifiable in `/tmp` during debugging.

- **`wrapper_path.exists()` TOCTOU.** The check at `executor.rs:111`
  and the spawn at line 130 are not atomic. The check is informational —
  `Spawn(ENOENT)` is the authoritative error. Worth a comment.

---

## Suggestions

- **Validate `cbscore_config_path` at worker startup**, not at build
  time. A misconfigured worker will accept builds from the server and
  fail every one. Failing at startup surfaces the problem immediately.

- **Emit a startup log line:** `cbscore-wrapper: starting build
  {version} trace_id={trace_id}` as the first stdout output.

- **`CBS_DRY_RUN=1` mode** that validates inputs but skips `runner()`.
  ~10 lines, enables CI testing without podman.

- **Use `json.dumps(result, separators=(',', ':'))`** for compact
  result JSON matching the `output.rs` prefix convention.

- **Add round-trip tests for all 4 `version_type` values** through the
  raw-dict path to confirm the lowercase serialization matches
  `get_version_type()` in cbscore.

---

## Strengths

- **All 6 prior blockers resolved.** The design has been iteratively
  refined through 3 review passes with no regressions.

- **Complete `version_create_helper()` call in step 6.** All 12
  parameters listed with exact JSON source expressions, serde rename
  notes, and `skip_serializing_if` behavior documented.

- **stderr → stdout redirect correctly placed and justified.** Step 1
  with `os.dup2(1, 2)` before imports prevents pipe deadlock and
  captures all diagnostic output.

- **`component_path` override is minimal and correct.** Reuses
  `_setup_components_dir()` without requiring cbscore changes.

- **Error classification table is actionable and complete.** Maps every
  failure mode to an exit code with specific error messages. Now
  includes `MalformedVersionError`.

- **SIGTERM limitation honestly documented.** Accepted tradeoff with
  clear justification (ephemeral `/tmp`, periodic container restarts).

- **JSON field name notes are precise.** The `"ref"` vs `"git_ref"`
  serde rename and `repo` absent-vs-null behavior are exactly the
  facts an implementer needs.

- **`cbsdcore` dependency removed.** Raw dict access eliminates an
  unnecessary install-time dependency.

---

## Open Questions

1. **Executor change sequencing:** One commit (wrapper + executor env
   vars) or two sequential commits? Either is defensible.

2. **`_tools/cbscore-entrypoint.sh` in wheel manifest:** Step 8
   validates its presence, but has it been verified that the cbscore
   wheel includes `_tools/` in `package_data`?

3. **`CBS_TRACE_ID` redundancy:** The wrapper inherits it from the
   executor. The stub sets it again from stdin. Which is authoritative
   if they ever differ? The design says the executor-set value (line
   159) — confirm this is intentional.

4. **Concurrent builds on a single worker:** Does the Rust worker
   prevent receiving a second `BuildNew` before the first completes?
   The design assumes single-build-per-worker. If this assumption holds,
   state it explicitly.
