# Design Review: 011 — Build Artifact Reporting (v1)

**Document:**
`design/011-20260321T0401-build-artifact-reporting.md`

---

## Summary

The end-to-end pipeline direction is sound and the
backwards-compatibility story is clean. However, the design
has 3 blockers that will make the feature silently do nothing
if implemented as written: the bind-mount report file path
is wrong (the proposed `/tmp/` is not mounted), the Rust
result-line parser must be explicitly extended to extract
`build_report`, and `BuildRecord`/`list_builds` need
coordinated schema updates. Additionally, the `skipped`
build path is critically underspecified.

**Verdict: Revise and re-review.**

---

## Blockers

### B1 — Report file path `/tmp/` is not bind-mounted

The design proposes `Builder.run()` writes to
`/tmp/build-report.json` inside the Podman container.
But `/tmp` inside the container is **not** in
`runner.py`'s `podman_volumes` dict. When the container
exits, the file vanishes. `runner()` on the host will
never find it.

The actually bind-mounted scratch path is
`/runner/scratch` (mapped from `config.paths.scratch`
on the host, runner.py line 260).

**Fix:** Change the report file path to
`/runner/scratch/build-report.json` inside the container
(maps to `{config.paths.scratch}/build-report.json` on
the host). `runner()` reads it from the host path after
`podman_run()` returns. Add a `finally` block to delete
it so stale reports don't pollute subsequent builds.

Update Open Question 1 to reflect this path. Confirm
that `Builder` has access to `config.paths.scratch`
inside the container (it does — the config remapping
in `runner.py` line 229 sets it to
`Path("/runner/scratch")`).

### B2 — `output.rs` parser must explicitly extract `build_report`

The current result-line parsing in `output.rs` manually
extracts `exit_code` and `error` from the parsed
`serde_json::Value`. The `WrapperResult` struct has only
those two fields. Adding `build_report` to the Python
output without updating the Rust extraction code means
the field is parsed into the JSON value and then
**silently discarded** — no code reads it.

The design mentions extending `WrapperResult` to add
`build_report: Option<serde_json::Value>` and changing
`stream_output()` to return a 3-tuple, but does not show
the extraction code:

```rust
build_report: parsed.get("build_report").cloned(),
```

If this line is omitted, the feature silently does nothing
at the worker layer.

**Fix:** Show the complete updated `WrapperResult` struct
AND the extraction code together in the design. This
closes the loop between Layer 2 (Python emits) and
Layer 3 (Rust extracts).

### B3 — `BuildRecord` and `list_builds` need coordinated updates

The design lists `cbsd-server/src/db/builds.rs` as changed
but doesn't specify:

1. `BuildRecord` must gain
   `pub build_report: Option<String>`.
2. `get_build()`'s `sqlx::query!` must SELECT the new
   column (compile-time checked — will fail if missing).
3. `list_builds()`'s hand-written SQL string must also
   be updated (no compile-time check — will silently
   omit the field).
4. `row_to_build_record()` must extract the new column.

If `BuildRecord` is updated but `list_builds` SQL is not,
`GET /api/builds/` silently omits `build_report` while
`GET /api/builds/{id}` includes it — inconsistent API.

Additionally, `BuildRecord` is serialized directly to
JSON. If `build_report` is `Option<String>`, the API
response will contain the report as a **quoted string**,
not a nested object. The design's example shows a nested
object. This requires either a separate `BuildResponse`
type or deserializing the TEXT column to
`serde_json::Value` before serialization.

**Fix:** Specify the exact `BuildRecord` changes, the
`list_builds` SQL update, and how the JSON string in the
DB becomes a nested object in the API response.

---

## Major Concerns

### M1 — `skipped=True` path is critically underspecified

The design says: "When `skipped` is true, the `components`
list and `release_descriptor` may still be populated if the
release was found in S3."

The actual code at `builder.py:112-114`:

```python
if skopeo_image_exists(container_img_uri, self.secrets):
    logger.info("image already exists -- do not build!")
    return
```

This is a bare `return None`. No S3 data is read. The
`check_release_exists()` call happens **after** this early
return and is only reached when the image does NOT exist.

**Fix:** Either (a) define the skipped report as minimal:
`skipped=True`, `container_image` populated,
`components=[]`, `release_descriptor=None`, or (b) add
an S3 lookup before the early return. Document the choice.

### M2 — `runner.py` report file read placement

`runner.py` lines 302-316: when `rc != 0`, it raises
`RunnerError` **before** any post-container code runs.
If the report file read is placed after the `rc` check,
partial reports (RPMs uploaded but container push failed)
are lost. If placed before, they're available.

**Fix:** Define the exact position of the report file
read relative to the `if rc != 0: raise RunnerError`
check. If partial reports are out of scope, say so and
ensure the file-read is unreachable on failed builds.

### M3 — `rpm_count` has no data source

`ComponentReport.rpm_count` is proposed but
`ReleaseComponentVersion.artifacts` is
`ReleaseRPMArtifacts(loc, release_rpm_loc)` — no count.
The RPM count would need to be computed from build output
inside the container, which is not surfaced.

**Fix:** Remove `rpm_count` from the initial design, or
document exactly where it's computed.

### M4 — No size limit on `build_report`

The report travels through stdout pipe → Rust parser →
WebSocket → server → SQLite. A compromised cbscore could
emit a 100 MB report. No layer enforces a size cap.

**Fix:** Enforce a max size at the worker (e.g., 64 KB)
before forwarding. Log and discard if exceeded. Document
the limit.

### M5 — `handle_build_finished` signature change

The design lists `dispatch.rs` as changed but doesn't
specify that `handle_build_finished` and
`set_build_finished` both need `build_report` parameters,
and the `.sqlx/` cache must be regenerated.

**Fix:** List the signature changes and `.sqlx/` update
explicitly.

---

## Minor Issues

- **`list_builds` returning `build_report` per row.**
  With hundreds of builds, each carrying KB of report
  JSON, the list endpoint becomes expensive. Consider
  excluding `build_report` from list responses (return
  it only on `GET /api/builds/{id}`).

- **Migration filename.** Should be
  `004_build_report.sql` following the convention
  (`001_...`, `002_...`, `003_...`).

- **Report file naming collision.** If concurrent builds
  share the same scratch directory (not currently
  possible — one build per worker), they'd collide on
  `build-report.json`. Safe for now; flag for future.

- **`model_dump(mode="json")` is correct Pydantic v2.**
  Returns a plain dict for JSON-safe output. Nesting in
  the outer `json.dumps` call is correct.

- **`ComponentReport.repo_url` source.** Verify
  `BuildComponentInfo.repo_url` exists at the point
  `_do_build_release` is called.

- **Result line compact JSON contract.** Document that
  the wrapper must always emit compact JSON
  (`separators=(",",":")`) and the Rust detection is
  hardcoded for that format.

---

## Suggestions

- **Consider `serde_json::value::RawValue`** in Rust
  instead of `serde_json::Value` for the worker. Avoids
  full parse-and-reserialize — just passes through the
  raw JSON bytes. The worker doesn't need the tree.

- **Add `build_report_version: 1`** to the report JSON
  for future schema evolution. When the report format
  changes, the server can handle old reports gracefully.

- **Add a Pydantic `BuildArtifactReport` test** that
  round-trips through `model_dump(mode="json")` +
  `json.dumps` + `json.loads` to verify the nesting
  works correctly.

---

## Strengths

- **Backwards compatibility is genuinely correct** —
  `Option`/`None`/`null` at every layer, independent
  deployment.

- **`serde_json::Value` in the worker is the right
  call** — decouples the worker from the report schema.

- **Option A (bind-mount) over Option B (stdout tag)
  is correct** — keeps structured data out of the log
  stream.

- **`ComponentReport` data sources are verified** —
  `ReleaseComponentVersion` carries the needed fields.

- **The end-to-end data flow diagram is accurate** for
  the current state.

---

## Open Questions

1. **Scratch path convention.** Confirm the report file
   is written to `/runner/scratch/build-report.json`
   inside the container and read from
   `config.paths.scratch / "build-report.json"` on host.

2. **`skipped` report content.** Minimal (just
   `container_image`) or enriched (S3 lookup for
   existing release descriptor)?

3. **Partial reports on failure.** Where does the file
   read go relative to `if rc != 0: raise RunnerError`?

4. **`rpm_count` source.** Does `ComponentBuild` carry
   a list of RPM paths? If so, `len(comp_build.rpms)`.
   If not, remove the field.

5. **`list_builds` API shape.** Should `build_report`
   be excluded from list responses? Recommend yes.

6. **Max report size.** What number (in bytes) is the
   limit at the worker layer?
