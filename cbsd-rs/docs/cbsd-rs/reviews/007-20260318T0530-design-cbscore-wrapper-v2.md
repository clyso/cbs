# Design Review: cbscore Wrapper v2 — Python Subprocess Bridge

**Document reviewed:**


- `cbsd-rs/docs/cbsd-rs/design/007-20260318T0716-cbscore-wrapper.md` (revised)


**Cross-referenced against:**

- `cbsd-rs/cbsd-worker/src/build/executor.rs`
- `cbsd-rs/cbsd-worker/src/build/output.rs`
- `cbsd-rs/cbsd-worker/src/config.rs`
- `cbsd-rs/cbsd-proto/src/build.rs`
- `cbscore/src/cbscore/runner.py`
- `cbscore/src/cbscore/versions/create.py`
- `cbsd/cbslib/worker/builder.py`
- `cbsd-rs/scripts/cbscore-wrapper.py`

---

## Summary

The revised design resolves all 4 blockers from the v1 review (async
`runner()`, `component_path` override, `cbscore_path` config, env vars
acknowledged). However, step 7 (`version_create_helper()` invocation) is
critically under-specified: 4 required parameters are missing
(`image_name`, `image_tag`, `user_name`, `user_email`), the
`component_refs` dict construction has a serde rename trap (`git_ref` →
`"ref"` in JSON), and `component_uri_overrides` construction is
unspecified. An implementer following step 7 as written will get a
`TypeError` on every build. Additionally, `MalformedVersionError` is
missing from the error classification table.

**Verdict: Revise step 7 to enumerate all 12 `version_create_helper()`
parameters with their exact JSON source expressions. Fix the error table.
Then the design is approvable.**

---

## Prior Blocker Disposition

| v1 Blocker | Status |
|---|---|
| B1 — `CBSCORE_CONFIG` never passed | Design resolved (acknowledges Rust change needed) |
| B2 — async `runner()` unacknowledged | Resolved (step 10: `asyncio.run()`) |
| B3 — `component_path` doesn't reach `runner()` | Resolved (step 5: override `config.paths.components`) |
| B4 — `cbscore_path` under-specified | Resolved (step 9: `CBSCORE_PATH` env + fallback + validation) |

---

## Blockers

### B1 — `version_create_helper()` invocation is fatally incomplete

Step 7 says: "`registry=config.storage.registry.url`, `el_version` from
step 6, `components_paths=config.paths.components` (overridden in step 5),
All other fields from the descriptor JSON."

`version_create_helper()` at `create.py:153-166` has **12 required
positional parameters**. Four are not named in step 7 and require
extraction from nested JSON sub-objects:

```python
image_name=descriptor["dst_image"]["name"],
image_tag=descriptor["dst_image"]["tag"],
user_name=descriptor["signed_off_by"]["user"],
user_email=descriptor["signed_off_by"]["email"],
```

An implementer relying on "all other fields" will not discover these
nested extractions. The call will fail with `TypeError: missing required
keyword argument`.

Additionally, two derived parameters require explicit construction:

- `component_refs={c["name"]: c["ref"] for c in descriptor["components"]}`
  — Note: the JSON key is `"ref"` (not `"git_ref"`) due to
  `#[serde(rename = "ref")]` at `build.rs:98`. An implementer who reads
  the Rust struct will try `c["git_ref"]` and get `KeyError`.

- `component_uri_overrides={c["name"]: c["repo"] for c in
  descriptor["components"] if c.get("repo") is not None}` — `repo` is
  omitted from JSON entirely when absent (Rust `skip_serializing_if`),
  so `c.get("repo")` is the correct check. An empty dict here causes
  builds to silently use wrong repos.

**Fix:** Replace "All other fields from the descriptor JSON" with the
complete explicit call:

```python
version_create_helper(
    version=descriptor["version"],
    version_type_name=descriptor["version_type"],
    component_refs={c["name"]: c["ref"] for c in descriptor["components"]},
    components_paths=config.paths.components,  # overridden in step 5
    component_uri_overrides={
        c["name"]: c["repo"]
        for c in descriptor["components"]
        if c.get("repo") is not None
    },
    distro=descriptor["build"]["distro"],
    el_version=el_version,  # from step 6
    registry=config.storage.registry.url,
    image_name=descriptor["dst_image"]["name"],
    image_tag=descriptor["dst_image"]["tag"],
    user_name=descriptor["signed_off_by"]["user"],
    user_email=descriptor["signed_off_by"]["email"],
)
```

### B2 — `MalformedVersionError` missing from error classification

`version_create_helper()` raises `MalformedVersionError` at
`create.py:223` for malformed version strings. This is distinct from
`VersionError` and `RunnerError`. The design's error table maps only
`RunnerError` and `VersionError` to exit 1. `MalformedVersionError`
falls through to "Unexpected exception → exit 2", misclassifying a
user-facing build error as an infrastructure failure.

**Fix:** Add to the error table:

| `MalformedVersionError` (bad version string) | 1 | Error message |

And note in step 12 that the exception handler must catch
`MalformedVersionError` alongside `RunnerError` and `VersionError`.

---

## Major Concerns

### M1 — `executor.rs` still only passes `CBS_TRACE_ID`

The design correctly lists the needed env vars and acknowledges the
Rust-side change. But `executor.rs:132` still has only:

```rust
.env("CBS_TRACE_ID", trace_id)
```

The wrapper commit and executor env-var commit must be sequenced. The
design should add a "Prerequisite Rust change" subsection making this
dependency explicit and specifying which config fields source each var.

Additionally, `CBSCORE_PATH` source is ambiguous: the env var table says
"wrapper path parent dir, or explicit config" but `cbscore_path` (the
library directory mounted into podman) and `cbscore_wrapper_path` (the
script) are different filesystem locations. Clarify that `CBSCORE_PATH`
points to the **cbscore library directory** (used as the volume mount
source), not the wrapper script directory.

### M2 — `executor.rs` pipes stderr but never reads it

`executor.rs:135` sets `.stderr(Stdio::piped())`. After the wrapper's
`os.dup2(1, 2)`, no bytes reach this pipe. Pre-dup2 failures (syntax
errors, import failures before `main()`) write to the real stderr pipe
but are never read — silently lost.

**Fix:** Either change to `Stdio::null()` (accept loss of pre-dup2
errors) or drain stderr in a separate tokio task and log via
`tracing::warn!`. The design should specify which approach.

### M3 — `CBS_TRACE_ID` ordering inconsistency in steps 2–3

Step 2 says "Set `CBS_TRACE_ID` environment variable (before any cbscore
imports)." Step 3 says "Read JSON from stdin" — but `trace_id` comes
from stdin. The env var is already set by the executor (`executor.rs:132`
does `.env("CBS_TRACE_ID", trace_id)`), so step 2 is redundant. Either
remove step 2 or clarify: "CBS_TRACE_ID is already set by the executor;
the wrapper does not need to set it again."

### M4 — SIGTERM bypasses `finally` blocks — temp files leak

Python's default SIGTERM handler calls `_exit()`, bypassing `finally`
blocks and atexit handlers. Every cancelled build will leak the
`VersionDescriptor` temp file from step 8. On a busy worker, these
accumulate in `/tmp`.

**Fix:** Either install a `signal.signal(signal.SIGTERM, ...)` handler
that sets a flag and lets `asyncio.run()` exit cleanly, or document that
temp file cleanup under SIGTERM is a known limitation and rely on `/tmp`
tmpwatch or container restarts.

---

## Minor Issues

- **Temp file needs `delete=False`.** Step 8 should specify
  `tempfile.NamedTemporaryFile(delete=False, suffix='.json')` or
  `tempfile.mkstemp()` + explicit close + `Path.unlink(missing_ok=True)`
  in the `finally` block. Default `NamedTemporaryFile` deletes on close,
  which happens before `runner()` reads the file.

- **Result line can be mis-detected by build output.** `output.rs:94`
  prefix-matches `{"type":"result"`. If RPM `%post` scripts or
  container layers emit a line starting with this prefix, it will be
  parsed as the wrapper result. Last-write-wins means the actual result
  line overwrites it only if it comes last. Consider using a unique
  sentinel prefix or only matching at EOF.

- **`run_name` format.** `trace_id[:8]` may include a hyphen if UUID
  is in standard format (`xxxxxxxx-xxxx-...`). Use
  `trace_id.replace("-", "")[:8]` for a clean container name suffix.

- **`VersionError` parenthetical.** The error table says "bad descriptor"
  but `VersionError` also covers invalid `version_type_name`. Minor
  wording issue.

- **`cbsdcore` dependency.** Listed in Dependencies for `BuildDescriptor`
  validation, but the design uses raw dict access throughout. Either
  remove `cbsdcore` from the dependency list or specify Pydantic
  validation as the canonical approach (which would eliminate the
  `"ref"` / `"git_ref"` trap in B1).

---

## Suggestions

- Emit a startup log line: `cbscore-wrapper: starting build
  {descriptor["version"]} trace_id={trace_id}`.

- Add `CBS_DRY_RUN=1` mode that validates all inputs but skips
  `runner()`. ~10 lines, enables CI testing without podman.

- Use `json.dumps(result, separators=(',', ':'))` for compact result
  JSON (convention match with the prefix in `output.rs`).

- Specify `flush=True` explicitly on the `print()` in `log_out_cb` to
  prevent buffering surprises.

---

## Strengths

- **All 4 original blockers resolved.** async `runner()`, component_path
  override, cbscore_path config, and env vars are all correctly specified.

- **stderr → stdout redirect placed first (step 1).** `os.dup2(1, 2)`
  before imports is the correct ordering.

- **Exit code 1/2 distinction is operationally useful.** Build failures
  vs. infrastructure failures, clear classification table.

- **`replace_run=True` explicitly included.** Prevents stale container
  hazard on worker restarts.

- **`component_path` override is minimal and correct.** Reuses existing
  `_setup_components_dir()` without requiring cbscore changes.

- **Process group isolation via `setsid()` correctly defers signal
  handling to the Rust worker.**

---

## Open Questions

1. **Pydantic vs. raw dict:** Should the wrapper validate the descriptor
   via `cbsdcore.BuildDescriptor` (type-safe, `.ref` attribute works
   naturally) or use raw dict access (simpler, but `"ref"` trap)? The
   choice resolves B1's `component_refs` concern and the `cbsdcore`
   dependency question.

2. **`_tools/cbscore-entrypoint.sh` in wheel:** Has it been verified
   that this file ships in the cbscore wheel's `package_data`? Step 9
   validates its presence, but if it's missing from the wheel manifest,
   every installed-from-wheel deployment fails.

3. **`CBSCORE_PATH` semantics:** Is it the cbscore library directory
   (for the podman volume mount) or the wrapper script directory? These
   are different. The fallback `Path(cbscore.__file__).parent` is the
   library directory — the env var name should match.

4. **Executor change commit sequencing:** One atomic commit (wrapper +
   executor env vars + `.sqlx/` cache if needed) or two sequential
   commits (executor first, wrapper second)?
