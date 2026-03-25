# Design Review: cbscore Wrapper — Python Subprocess Bridge

**Document reviewed:**


- `cbsd-rs/docs/cbsd-rs/design/007-20260318T0716-cbscore-wrapper.md`


**Cross-referenced against:**

- `cbsd-rs/cbsd-worker/src/build/executor.rs`
- `cbsd-rs/cbsd-worker/src/build/output.rs`
- `cbsd-rs/scripts/cbscore-wrapper.py`
- `cbscore/src/cbscore/runner.py`
- `cbscore/src/cbscore/versions/create.py`
- `cbsd/cbslib/worker/builder.py`

---

## Summary

The design correctly identifies the subprocess bridge problem, the
stdin/stdout wire format, and the division of responsibilities between
the Rust worker and Python wrapper. The interface contract matches what
the executor already implements. However, the design has **4 blockers**
that will cause the implementation to fail if not addressed: two missing
environment variables that the executor never passes to the subprocess,
the async nature of `runner()` being unacknowledged, and a fundamental
misunderstanding of how `component_path` reaches the build container.
The design needs a revision pass but not a rethink.

---

## Blockers

### B1 — `CBSCORE_CONFIG` is never passed to the subprocess

The design says the wrapper loads config from `CBSCORE_CONFIG` env var
(step 3). The worker config parses `cbscore_config_path` into
`ResolvedWorkerConfig` (`config.rs:52`). But `executor.rs:130-132` only
sets one env var on the subprocess:

```rust
cmd.arg(&wrapper_path)
    .env("CBS_TRACE_ID", trace_id)
    .stdin(Stdio::piped())
```

`CBSCORE_CONFIG` is never `.env()`-set. The wrapper will always fall
through to the fragile cwd fallback, which will fail in production.

**Fix:** The executor must be updated to pass `CBSCORE_CONFIG` from
`ResolvedWorkerConfig.cbscore_config_path`. This is a Rust-side change
that the design claims is unnecessary ("What changes on the Rust side:
Nothing"). The design must acknowledge this prerequisite or the wrapper
will fail at step 3 in every deployment.

### B2 — `runner()` is async — the design never mentions `asyncio.run()`

`runner()` in `cbscore/runner.py` is `async def runner(...)`. The design
step 6 says "Call `cbscore.runner.runner()`" without acknowledging this.
A direct synchronous call returns a coroutine object — no build executes,
no error is raised, and the wrapper exits 0 (success).

**Fix:** Add to the design: "The wrapper must call
`asyncio.run(runner(...))` because `runner()` is an async function.
The `log_out_cb` must be `async def` (matching cbscore's
`AsyncRunCmdOutCallback` type alias). A synchronous `print()` inside
the async callback is acceptable for a single-threaded process."

### B3 — `component_path` from stdin does not flow through to `runner()`

The design says the wrapper "receives `component_path` pointing to an
already-unpacked directory" and implies this is passed to
`version_create_helper()` as `components_paths`. But `runner()` at line
208 calls `_setup_components_dir(config.paths.components)` — hardcoded
to the config object, not any parameter. The wrapper's stdin
`component_path` has no corresponding parameter in `runner()`.

The wrapper must override `config.paths.components` in memory before
calling both `version_create_helper()` and `runner()`:

```python
config.paths.components = [Path(component_path)]
```

Without this, `runner()` will mount the config's original
`components_paths` into the build container, ignoring the tarball the
Rust worker unpacked. The build either fails (paths don't exist) or
builds the wrong component (stale paths from a previous run).

**Fix:** State explicitly: "The wrapper must set
`config.paths.components = [Path(component_path)]` before calling
`version_create_helper()` or `runner()`. This is the mechanism by which
the Rust worker's unpacked tarball reaches the build container."

Also clarify the directory structure: `component_path` should be the
*parent* directory containing a named component subdirectory (e.g.,
`/tmp/build-abc123/` containing `ceph/`), because
`_setup_components_dir()` iterates subdirectories.

### B4 — `cbscore_path` semantics are under-specified

`runner()` takes `cbscore_path: Path` as a mandatory positional argument.
It mounts this directory into the podman container at `/runner/cbscore`
(`runner.py:256`). The entrypoint script inside the container runs the
build from this mount.

The design says "derived from the cbscore package location" but provides
no config mechanism. The Python worker (`cbsd/cbslib/config/worker.py:35`)
has `cbscore_path` as an explicit required config field.

In production containers where cbscore is installed via `uv`,
`Path(cbscore.__file__).parent` resolves to `site-packages/cbscore/`.
Whether `_tools/cbscore-entrypoint.sh` is included in that installation
depends on the wheel manifest — this is unverified.

**Fix:** Add a `CBSCORE_PATH` environment variable (same pattern as
`CBSCORE_CONFIG`). The executor must set it from config. The wrapper
falls back to `Path(cbscore.__file__).parent` if unset, with a
validation check that `_tools/cbscore-entrypoint.sh` exists at the
expected location. Document both the env var and fallback behavior.

---

## Major Concerns

### M1 — `os_version` → `el_version` integer conversion is missing

`version_create_helper()` takes `el_version: int`. The `BuildDescriptor`
carries `build.os_version: str` (e.g., `"el9"`). The Python worker
parses this with `re.match(r"^el(\d+)$", ...)` and raises on mismatch.

The design's step 4 does not mention this parse step. An implementer
will either pass the string (TypeError at runtime) or parse it ad-hoc
without proper error classification.

**Fix:** Add explicit sub-step: "Parse `build.os_version` with
`^el(\d+)$`. If it doesn't match, emit exit code 2 ('invalid os_version:
{value}'). Extract the integer and pass as `el_version=`."

### M2 — `registry` parameter source not specified

`version_create_helper()` requires `registry=config.storage.registry.url`.
This comes from the cbscore config, not the `BuildDescriptor`. The
Python worker guards: `if not config.storage or not config.storage.registry:
raise`.

The design step 4 does not map this parameter.

**Fix:** Add: "After loading config, verify `config.storage.registry` is
present (exit code 2 if missing). Pass
`registry=config.storage.registry.url` to `version_create_helper()`."

### M3 — `CBS_BUILD_TIMEOUT` is never set by the executor

The design says the wrapper reads timeout from `CBS_BUILD_TIMEOUT` env
var. Like `CBSCORE_CONFIG`, this env var is never set by `executor.rs`.
The wrapper will always use the default (2h/7200s), ignoring any
operator-configured timeout.

**Fix:** Either add `CBS_BUILD_TIMEOUT` to the executor's `.env()` calls
(from worker config), or document that timeout is always the default
and add the env var support to the executor as a follow-up.

### M4 — stderr is piped but never read — deadlock risk

The executor pipes stderr (`Stdio::piped()`, line 135) but `output.rs`
only reads stdout. If cbscore or podman writes >64KB to stderr (a Python
traceback, podman error output, or cbscore's own `logging.basicConfig()`
output which goes to stderr), the pipe buffer fills and the subprocess
blocks on `write(2, ...)`. The Rust side waits for stdout EOF, which
never arrives. The build hangs until the executor's timeout fires.

Additionally, all cbscore diagnostic logging (`logger.debug/info/warn`)
goes to stderr via Python's logging framework — this output is silently
lost even in the non-deadlock case.

**Fix:** Either: (a) the wrapper redirects stderr to stdout
(`sys.stderr = sys.stdout` or `os.dup2(1, 2)`) so all output is
captured, or (b) the executor changes stderr to `Stdio::null()` to
prevent the deadlock (accepting loss of diagnostics), or (c) the
executor spawns a second task to drain stderr. Option (a) is simplest
and preserves diagnostic information.

---

## Minor Issues

- **`VersionError` not in error classification table.** Raised by
  `version_create_helper()` for malformed versions or missing components.
  Falls through to the `Exception` catchall (exit 2), but the Python
  worker treats it as a build error (exit 1). Classify explicitly.

- **`replace_run=True` flag unaddressed.** The Python worker passes this
  to handle stale containers from previous runs. The design should
  specify `replace_run=True`.

- **`--config` CLI argument never sent by executor.** The design mentions
  it as an alternative to `CBSCORE_CONFIG`, but the executor only passes
  the script path. Clarify this is for manual testing only.

- **`CBS_TRACE_ID` should be set before cbscore imports.** If cbscore's
  logger reads the env var during `logging.basicConfig()` at import time,
  setting it after import misses early messages. Set it as the first
  operation in the wrapper.

- **`run_name` length.** UUID v4 is 36 chars; with `cbs-` prefix = 40.
  Podman accepts this, but the Python worker uses short names
  (`gen_run_name("ces_")`) for historical reasons. Use the truncated
  form `cbs-{trace_id[:8]}` as shown in the plan, not the full UUID.

- **Config fallback is operationally fragile.** The cwd fallback will
  always fail in deployed workers. Consider making `CBSCORE_CONFIG`
  mandatory and removing the fallback, or documenting it as dev-only.

---

## Suggestions

- Add a startup log line before the build: `cbscore-wrapper: starting
  build trace_id=... version=...` — useful for build log readability
  and matching log entries to traces.

- Consider a dry-run mode (`CBS_DRY_RUN=1`) that validates inputs but
  skips `runner()`. Costs ~10 lines, enables independent wrapper testing
  without podman.

- Use `json.dumps(result, separators=(',', ':'))` for the result line
  to emit compact JSON. The Rust parser handles both formats, but
  compact is the convention established by the prefix match in
  `output.rs:94`.

---

## Strengths

- **Wire format is correct and validated.** The `{"type":"result",...}`
  protocol exactly matches `output.rs` parsing. The dual-prefix
  detection accommodates both compact and spaced JSON.

- **Responsibility boundaries are well-drawn.** The "What the wrapper
  does NOT do" section prevents scope creep and keeps the wrapper
  focused.

- **Exit code classification is sound.** The 0/1/2 scheme maps cleanly
  to `classify_exit_code()` in the executor. Signal exits (137/143)
  are correctly handled as `Revoked` on the Rust side.

- **Error table covers meaningful failure modes.** `RunnerError`,
  `ConfigError`, import failure, stdin parse, and catchall — gives
  implementers a complete mapping.

- **Signal handling correctly deferred.** The wrapper and podman live
  in the process group from `setsid()`. SIGTERM/SIGKILL is the Rust
  worker's responsibility. No special handling needed in the wrapper.

---

## Open Questions

1. **`cbscore_path` in production containers:** Has anyone verified that
   `_tools/cbscore-entrypoint.sh` is present inside
   `site-packages/cbscore/` when installed via `uv`? `runner.py:122-128`
   derives it from `Path(__file__).parent / "_tools" / ...`.

2. **`component_path` directory structure:** The wrapper receives a path.
   `_setup_components_dir()` iterates subdirectories. If the Rust worker
   unpacks to `/tmp/build-abc/ceph/`, should `component_path` be
   `/tmp/build-abc/` (parent) or `/tmp/build-abc/ceph/` (component)?
   Must match cbscore's expectation of `{component_path}/{component_name}/`.

3. **Scope of Rust-side changes:** The design claims "Nothing" changes
   on the Rust side. Blockers B1 and B4 require executor changes (passing
   `CBSCORE_CONFIG` and `CBSCORE_PATH` env vars). Should these be a
   separate commit or part of the wrapper commit?
