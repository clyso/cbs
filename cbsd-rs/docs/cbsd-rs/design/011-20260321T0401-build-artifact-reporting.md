# 011 — Build Artifact Reporting

## Status

Draft v2 — addresses review v1

## Problem

After a build completes, the Rust worker receives only an exit
code and an optional error string from the cbscore subprocess.
No structured information about the produced artifacts is
propagated — not which container images were pushed, not which
RPM packages were uploaded, not where the release descriptor
landed in S3. The server stores `success` or `failure` but has
no record of what was actually produced.

This means:

- Operators must query S3 manually to find artifact locations.
- Downstream automation (deploy, test, release pipelines)
  cannot be triggered with artifact metadata from the build
  system.
- The hook system (design 012) cannot include artifact details
  in `build.succeeded` event payloads.
- There is no way to answer "what did build 42 produce?"
  from the CBS API alone.

## Current Data Flow

```
                         cbscore-wrapper.py
                               │
         writes descriptor ──→ │ ──→ runner() ──→ Podman
         to stdin (JSON)       │         │
                               │    Builder.run()
                               │    _do_build_release()
                               │         │
                               │    ReleaseDesc constructed
                               │    (uploaded to S3, then
                               │     discarded in memory)
                               │
                               │ ←── log lines (stdout)
                               │
                               │ ←── result line:
                               │     {"type":"result",
                               │      "exit_code":0,
                               │      "error":null}
                               │
     Rust worker ←─────────────┘
         │
         ├─ stream_output() parses result line
         │  → (BuildFinishedStatus, Option<String>)
         │
         ├─ sends WorkerMessage::BuildFinished
         │  {build_id, status, error}
         │
     Server ←──────────────────┘
         │
         └─ stores state + error in builds table
```

The `ReleaseDesc` — which contains component versions, git
SHAs, S3 artifact paths, and RPM metadata — is constructed
inside `Builder._do_build_release()` (builder.py line 211-317)
and returned to `Builder.run()`, where it is used to build the
container image and then discarded. It is never serialized
back to the caller.

## Design

### Approach: Structured report via wrapper stdout

Extend the wrapper's result JSON line to include a
`build_report` field. This propagates through the existing
stdout → Rust parser → WebSocket → server pipeline with
minimal changes at each layer.

### Layer 1: cbscore (`builder.py`)

`Builder.run()` currently returns `None`. Change it to return
an `Optional[BuildArtifactReport]` — a new Pydantic model
that summarizes what was produced.

```python
class BuildArtifactReport(pydantic.BaseModel):
    """Summary of artifacts produced by a build."""

    report_version: int = 1
    version: str
    skipped: bool

    container_image: ContainerImageReport | None
    release_descriptor: ReleaseDescriptorReport | None
    components: list[ComponentReport]


class ContainerImageReport(pydantic.BaseModel):
    name: str           # "harbor.clyso.com/ces-devel/ceph"
    tag: str            # "v19.2.3-dev.1"
    pushed: bool


class ReleaseDescriptorReport(pydantic.BaseModel):
    s3_path: str        # "releases/19.2.3.json"
    bucket: str         # "cbs-releases"


class ComponentReport(pydantic.BaseModel):
    name: str           # "ceph"
    version: str        # "19.2.3-42.g5a0b003"
    sha1: str           # git commit hash
    repo_url: str       # source repository URL
    rpms_s3_path: str | None    # S3 path to RPMs
```

`report_version` is included for future schema evolution.
Consumers check this field to handle old/new report formats.

**Where the data comes from:**

- `container_image`: from `get_container_canonical_uri()` and
  `skopeo_image_exists()` after push (builder.py line ~170).
- `release_descriptor`: from `release_desc_upload()` return
  value (builder.py line ~302-311).
- `components`: from `_do_build_release()`'s
  `ReleaseComponentVersion` dict (builder.py line ~294-299).
- `skipped`: `True` when the container image already existed
  and no build was performed (builder.py line ~113-114).

**When `skipped` is true**, the early return at builder.py
line 114 (`return None`) fires before any S3 data is read.
The skipped report is therefore minimal:

```python
BuildArtifactReport(
    version=self.desc.version,
    skipped=True,
    container_image=ContainerImageReport(
        name=container_img_uri.name,
        tag=container_img_uri.tag,
        pushed=False,
    ),
    release_descriptor=None,
    components=[],
)
```

No S3 lookup is added for the skipped path. The image
already exists, so the caller knows what was produced from
the descriptor alone.

**Return flow:**

```
_do_build_release() → ReleaseDesc
                          ↓
Builder.run()         → BuildArtifactReport
                          ↓
  writes report to /runner/scratch/build-report.json
                          ↓
runner()              → reads report from host-side path
```

### Layer 1b: cbscore (`runner.py`)

The `runner()` function spawns a Podman container. The
`Builder` class executes inside the container. Structured
data must cross the container boundary via the filesystem.

**Report file path.** The builder writes the report to
`/runner/scratch/build-report.json` inside the container.
This path is bind-mounted from `config.paths.scratch` on
the host (runner.py line 260). The runner reads it from
`config.paths.scratch / "build-report.json"` on the host
after `podman_run()` returns.

A `finally` block deletes the report file on the host to
prevent stale reports from polluting subsequent builds:

```python
report_host_path = config.paths.scratch / "build-report.json"
try:
    rc, _, stderr = await podman_run(...)
except PodmanError as e:
    ...
finally:
    _cleanup_components_dir(components_path)

# Read report BEFORE the rc check — captures partial
# reports when RPMs uploaded but container push failed.
report: BuildArtifactReport | None = None
try:
    if report_host_path.exists():
        raw = report_host_path.read_text()
        report = BuildArtifactReport.model_validate_json(raw)
finally:
    report_host_path.unlink(missing_ok=True)

if rc != 0:
    msg = f"error running build (rc={rc}): {stderr}"
    logger.error(msg)
    raise RunnerError(msg)

return report
```

The report file read is placed **before** the `rc != 0`
check. This means partial reports are available when the
build partially succeeded (e.g., RPMs uploaded but container
push failed). The wrapper can choose to include or discard
the partial report. For failed builds, the wrapper receives
a `RunnerError` exception — the report is lost via the
exception path. To surface partial reports on failure, the
exception would need to carry the report — this is deferred
to a future iteration.

`runner()` return type changes from `None` to
`Optional[BuildArtifactReport]`.

### Layer 2: cbscore-wrapper.py

After `runner()` returns, the wrapper includes the report
in the result line. The wrapper must always emit compact
JSON (`separators=(",",":")`) — the Rust result-line
detection is hardcoded for `{"type":"result"` prefix
matching.

```python
report = asyncio.run(runner(...))
report_dict = (
    report.model_dump(mode="json") if report else None
)
result = {
    "type": "result",
    "exit_code": 0,
    "error": None,
    "build_report": report_dict,
}
print(json.dumps(result, separators=(",", ":")), flush=True)
```

The `build_report` field is `null` when the build fails
(wrapper catches `RunnerError`), when it was revoked, or
when cbscore predates this change.

### Layer 3: Rust worker (`output.rs`)

Extend `WrapperResult` and the extraction code:

```rust
#[derive(Debug)]
struct WrapperResult {
    exit_code: i32,
    error: Option<String>,
    build_report: Option<serde_json::Value>,
}
```

The extraction code (currently at output.rs line 93-111)
must explicitly extract the new field:

```rust
if let Ok(parsed) =
    serde_json::from_str::<serde_json::Value>(&line)
{
    wrapper_result = Some(WrapperResult {
        exit_code: parsed
            .get("exit_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1) as i32,
        error: parsed
            .get("error")
            .and_then(|v| v.as_str())
            .map(String::from),
        build_report: parsed
            .get("build_report")
            .cloned(),
    });
}
```

Without the `build_report: parsed.get(...).cloned()` line,
the field is parsed into the JSON value and silently
discarded.

**Size limit.** Before forwarding, the worker checks the
serialized size of `build_report`. If it exceeds 64 KB, the
report is logged as a warning and discarded (set to `None`).
This prevents a compromised cbscore from sending oversized
payloads through the WebSocket to the server.

```rust
if let Some(ref report) = wrapper_result.build_report {
    let size = serde_json::to_string(report)
        .map(|s| s.len())
        .unwrap_or(0);
    if size > 65_536 {
        tracing::warn!(
            size, "build report exceeds 64 KB, discarding"
        );
        wrapper_result.build_report = None;
    }
}
```

`stream_output()` return type changes to:

```rust
Result<
    (BuildFinishedStatus, Option<String>, Option<Value>),
    OutputError,
>
```

### Layer 4: WebSocket protocol (`cbsd-proto`)

Add an optional field to `WorkerMessage::BuildFinished`:

```rust
BuildFinished {
    build_id: BuildId,
    status: BuildFinishedStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_report: Option<serde_json::Value>,
},
```

The field is `Option` with `skip_serializing_if` — older
workers that don't send it are deserialized with `None` via
`#[serde(default)]`. No protocol version bump needed.

### Layer 5: Server storage and API

**Database.** New migration (`004_build_report.sql`):

```sql
ALTER TABLE builds ADD COLUMN build_report TEXT;
```

**`handle_build_finished` signature change.** In
`dispatch.rs`, `handle_build_finished()` and
`set_build_finished()` both gain a
`build_report: Option<&str>` parameter. The report is
serialized to a JSON string before storage.

The `.sqlx/` offline cache must be regenerated after the
migration and query changes.

**`BuildRecord` changes** in `db/builds.rs`:

```rust
pub struct BuildRecord {
    // ... existing fields ...
    pub build_report: Option<serde_json::Value>,
}
```

The `build_report` column is `TEXT` in SQLite but
deserialized to `serde_json::Value` in `row_to_build_record`
so the API response contains a nested JSON object (not a
quoted string):

```rust
fn row_to_build_record(r: SqliteRow) -> BuildRecord {
    // ... existing fields ...
    let report_str: Option<String> = r.get("build_report");
    let build_report = report_str
        .and_then(|s| serde_json::from_str(&s).ok());
    BuildRecord { ..., build_report }
}
```

Both `get_build()` and `list_builds()` SQL queries must
SELECT the new column. `list_builds` uses a hand-written
SQL string (no compile-time check), so this is easy to miss.

**API response.** `GET /api/builds/{id}` includes the full
report. `GET /api/builds` (list endpoint) **excludes**
`build_report` to avoid expensive responses with hundreds
of builds each carrying KB of report JSON. The list endpoint
uses a separate query or response type that omits the
column.

```json
{
  "id": 123,
  "state": "success",
  "descriptor": { ... },
  "build_report": {
    "report_version": 1,
    "version": "19.2.3",
    "skipped": false,
    "container_image": {
      "name": "harbor.clyso.com/ces-devel/ceph",
      "tag": "v19.2.3-dev.1",
      "pushed": true
    },
    "release_descriptor": {
      "s3_path": "releases/19.2.3.json",
      "bucket": "cbs-releases"
    },
    "components": [
      {
        "name": "ceph",
        "version": "19.2.3-42.g5a0b003",
        "sha1": "5a0b003592a...",
        "repo_url": "https://github.com/ceph/ceph.git",
        "rpms_s3_path": "rpms/ceph/19.2.3-42/"
      }
    ]
  }
}
```

For non-success builds or builds without a report, the
field is `null`.

## Backwards Compatibility

Every layer uses `Option` / `None` / `null` for the new
field. This means:

- Old workers (no `build_report` in `BuildFinished`) →
  server sees `None` → stores `NULL` in DB. No breakage.
- Old wrapper (no `build_report` in result line) →
  worker parses `None` → forwards `None`. No breakage.
- Old cbscore (no `BuildArtifactReport` return) →
  wrapper gets `None` → emits `null`. No breakage.

Each layer can be deployed independently without
coordinating a simultaneous upgrade.

## Files Changed

### Python (cbscore + wrapper)

| File | Change |
|------|--------|
| `cbscore/src/cbscore/builder/builder.py` | Return `BuildArtifactReport` from `run()` |
| `cbscore/src/cbscore/builder/report.py` | New: Pydantic models |
| `cbscore/src/cbscore/runner.py` | Return report; read from scratch path |
| `cbsd-rs/scripts/cbscore-wrapper.py` | Include report in result JSON |

### Rust (worker + server)

| File | Change |
|------|--------|
| `cbsd-proto/src/ws.rs` | Add `build_report` to `BuildFinished` |
| `cbsd-worker/src/build/output.rs` | Extract + size-limit report |
| `cbsd-worker/src/ws/handler.rs` | Forward report in message |
| `cbsd-server/src/ws/dispatch.rs` | `handle_build_finished` + `set_build_finished` gain report param |
| `cbsd-server/src/db/builds.rs` | `BuildRecord` gains `build_report: Option<Value>`; `row_to_build_record` deserializes TEXT→Value; `list_builds` SQL updated; list response excludes report |
| `cbsd-server/migrations/004_build_report.sql` | `ALTER TABLE builds ADD COLUMN build_report TEXT` |
| `cbsd-server/src/routes/builds.rs` | Single-build response includes report |
| `.sqlx/` | Regenerated after migration + query changes |
