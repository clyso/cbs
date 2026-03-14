# cbsd Rust Port — Architecture & Task Queue Design

## Overview

This document describes the architecture for reimplementing the CBS daemon
(`cbsd/`) in Rust (2024 edition), replacing the current Python 3 + FastAPI +
Celery stack. The primary motivation is to eliminate Celery (which is
synchronous Python, underutilized, and adds unnecessary complexity), simplify
the architecture, and make the server the single source of truth for components.

## Current Architecture (Python)

```
┌─────────────────────┐         ┌──────────────┐
│  cbsd server        │  Redis  │ celery worker │
│  (FastAPI+Uvicorn)  │◄───────►│ (cbscore)     │
│                     │ broker  │               │
│  - REST API         │ results │ - build task  │
│  - OAuth/PASETO     │ streams │ - podman exec │
│  - build tracker    │         │ - log capture │
│  - event monitor    │         │               │
│  - periodic tasks   │         │               │
└─────────────────────┘         └──────────────┘
```

Problems:

- Celery is sync Python; running async code requires workarounds.
- Only one worker is in use; Celery's complexity is unjustified.
- Components live on workers; server must query workers at startup to discover
  them. Updates require restarting workers then server.
- The Redis broker, result backend, event monitoring, and custom Kombu
  serializer are all Celery machinery that would go away.

## Target Architecture (Rust)

```
┌──────────────────────┐    persistent    ┌────────────────┐
│  cbsd server (Rust)  │◄── WebSocket ───►│  worker (Rust)  │
│                      │   (worker→server │                 │
│  - REST API (axum)   │    initiated)    │  - WS client    │
│  - OAuth/PASETO      │                  │  - cbscore exec │
│  - build queue       │                  │  - podman build │
│  - build tracker     │                  │                 │
│  - log storage       │                  │                 │
│  - periodic tasks    │                  │                 │
│  - component store   │                  │                 │
└──────────────────────┘                  └────────────────┘
```

Key changes:

- **No Redis.** The server owns the queue, tracker, and log storage directly.
- **No Celery.** Replaced by a single persistent WebSocket per worker.
- **Server owns components.** Workers receive component tarballs with each
  build request.
- **Workers are pure clients.** They connect outbound to the server — no
  listening port, no firewall concerns, no TLS cert management on the worker
  side.

## Server Capabilities

The Rust server retains all current `cbsd` server capabilities:

| Capability | Rust Crate(s) | Notes |
|-----------|--------------|-------|
| HTTP REST API | `axum`, `tower`, `tokio` | Route-for-route port from FastAPI |
| SSL/TLS | `axum-server`, `rustls` | Built-in |
| Google OAuth 2.0 | `reqwest`, manual OIDC flow | ~200 LoC for 3-legged flow |
| PASETO v4 tokens | `pasetors` | Wire-compatible with current tokens |
| Capability-based authz | String enums in SQLite | RBAC model, see auth design doc |
| Session middleware | `tower-sessions` | OAuth state only |
| Shared API types (cbsdcore) | `serde`, `serde_json`, `chrono` | ~13 Pydantic models → serde structs |
| Configuration | `serde_yaml`, `serde_json` | Same YAML/JSON config format |
| Database (all state) | `sqlx` + SQLite | Single `cbsd.db`, compile-time checked queries |
| In-memory build tracker | `tokio::sync::RwLock`, `HashMap` | Hot cache backed by SQLite |
| Build log storage | `tokio::fs` | Server writes logs directly (no Redis streams) |
| Periodic/cron builds | `cron`, `tokio-cron-scheduler` | Design TBD (separate doc) |
| Logging | `tracing`, `tracing-subscriber` | Structured logging |
| Component store | Filesystem + in-memory index | Server is single source of truth |
| Build queue + dispatch | Custom, in-process | Matches workers by arch/resources |
| Worker management | WebSocket (axum built-in) | Per-worker persistent connection |

## Component Distribution

### Current problem

Components (`components/` directory) contain YAML descriptors, bash scripts,
patches, repo files, and per-version container definitions. Currently they live
on the worker's filesystem, volume-mounted into the worker container. The server
discovers them by querying workers at startup. Any update requires restarting
workers then server.

### New model: server as single source of truth

The server owns the component store. On each build dispatch, the server packs
the relevant component directory into a gzip tarball and sends it to the worker
alongside the build descriptor.

**Payload size:** Components are small (~6 KB gzipped per component, ~62 files
total across all components currently). This is trivially sendable over
WebSocket.

**Transfer mechanism:** WebSocket binary frames. The server sends a JSON text
frame (`build_new` message) followed by a binary frame containing the raw
tar.gz bytes. No base64 encoding needed — WebSocket natively supports binary
frames.

**Worker behavior:** Unpack tarball to a temp directory, pass that path to
cbscore for the build, clean up on completion.

**Future:** Components can be managed through a web interface or API without
touching workers at all.

## Worker ↔ Server Communication

### Connection model

Each worker maintains a single persistent WebSocket connection to the server,
initiated by the worker. This connection serves as:

1. **Command channel** — server dispatches builds, workers report status
2. **Log stream** — workers send build output in batched messages
3. **Heartbeat** — connection liveness implies worker liveness

### Worker startup

The worker receives the server's URL and an API key via command-line argument
or environment variable. On startup:

1. Connect to `wss://<server>/api/ws/worker` with
   `Authorization: Bearer <api-key>` header on the HTTP upgrade request.
2. Server validates the API key in the HTTP upgrade handler (axum extractor)
   **before** accepting the WebSocket. Invalid keys receive HTTP 401 and never
   get a WebSocket connection. This prevents unauthenticated connections from
   holding open sockets. The API key is **never** passed in the URL query
   string — query strings are logged by reverse proxies and access logs.
3. Server accepts the upgrade; WebSocket is now established.
4. Worker sends `hello` with protocol version and worker capabilities.
5. Server validates and responds with `welcome`.
6. Worker is now in the `idle` state, ready to receive builds.

### Authentication

Workers are trusted infrastructure, not human users. They use long-lived API
keys (see auth design doc). The API key is validated at the HTTP layer during
WebSocket upgrade — there is no authentication field in the WebSocket protocol
messages themselves.

### TLS configuration

The worker connects via `wss://` using `rustls`. By default, the OS trust
store is used for certificate validation. For internal deployments with
self-signed certificates or private CAs:

```yaml
# worker config
tls_ca_bundle_path: /etc/cbsd/ca-bundle.pem  # optional
```

When set, the PEM file is loaded and added as a trusted root to the rustls
`ClientConfig`. Without this, the first worker in a dev/lab deployment with
internal PKI will refuse to connect.

## WebSocket Protocol

JSON text frames for structured messages. Binary frames for component data.
All messages have a `type` field as discriminator.

### Server → Worker

```jsonc
// Dispatch a build. Followed by a binary frame containing the component tar.gz.
{
  "type": "build_new",
  "build_id": 42,
  "trace_id": "abc123def456",    // for cross-boundary log correlation
  "priority": "normal",
  "descriptor": { /* BuildDescriptor */ },
  "component_sha256": "e3b0c44..."  // SHA-256 of the binary frame that follows
}
// Binary frame: <raw tar.gz bytes of component directory>

// Cancel a build (running or not yet accepted).
{
  "type": "build_revoke",
  "build_id": 42
}
// If the worker receives build_revoke before it has sent build_accepted
// (e.g., still unpacking the component tarball), it immediately responds
// with build_finished(revoked) without starting execution.

// Connection accepted. connection_id is the server-assigned UUID for this
// worker connection — enables GET /workers correlation with worker-side tracing.
// grace_period_secs: worker validates its backoff ceiling against this value.
{
  "type": "welcome",
  "protocol_version": 1,
  "connection_id": "550e8400-e29b-41d4-a716-446655440000",
  "grace_period_secs": 90
}

// Connection rejected (includes server's supported range for diagnostics).
{
  "type": "error",
  "reason": "unsupported protocol version 3; server supports 1",
  "min_version": 1,
  "max_version": 1
}
```

**Integrity check:** The worker computes SHA-256 of the received binary frame
and compares it against `component_sha256`. If they don't match, the worker
sends `build_rejected` with `reason: "component integrity check failed"`.

**Server response to integrity failure:** Unlike other rejections, an integrity
failure indicates a server-side tarball corruption — re-queuing to another
worker would produce the same failure. The server marks the build `FAILURE`
with `error = "component integrity check failed"` and logs a server-side alarm.
The build is **not** re-queued.

### Worker → Server

```jsonc
// First message after WebSocket connect (auth already validated at HTTP
// upgrade). Combines protocol negotiation and capability registration.
// worker_id is a human-readable display label only — the server keys its
// internal workers map by a server-assigned opaque connection handle (UUID),
// not by worker_id. This prevents split-identity issues when two connections
// advertise the same worker_id.
{
  "type": "hello",
  "protocol_version": 1,
  "worker_id": "worker-arm64-01",    // display label, not identity key
  "arch": "x86_64",                  // known values: "x86_64", "aarch64" (alias: "arm64")
  "cores_total": 16,
  "ram_total_mb": 65536
}
// Future-proofing note: adding fields like cores_available, ram_available_mb,
// or build_slots_used to hello avoids a protocol version bump for load-aware
// dispatch later. Server ignores unknown fields (serde default).

// Sent on reconnect ONLY if the worker is currently executing a build.
// Its absence after hello implies the worker is idle.
// Reconnection reconciliation matches by build_id, not worker_id.
{
  "type": "worker_status",
  "state": "building",
  "build_id": 42
}

// Worker will run the build.
{
  "type": "build_accepted",
  "build_id": 42
}

// Worker cannot run the build (busy, incompatible, integrity failure, etc).
{
  "type": "build_rejected",
  "build_id": 42,
  "reason": "worker busy"
}

// Build execution has started (container launched).
{
  "type": "build_started",
  "build_id": 42
}

// Build output. Batched: flushed every 200ms or 50 lines, whichever first.
// Lines are UTF-8; non-UTF-8 bytes are replaced with U+FFFD. ANSI escape
// codes pass through (clients that want plain text must strip them).
// Newlines are stripped before inclusion in the lines array.
// Per-line seq: start_seq, start_seq+1, ..., start_seq+len(lines)-1.
{
  "type": "build_output",
  "build_id": 42,
  "start_seq": 70,              // per-build monotonic, per-line granularity
  "lines": ["line 1", "line 2", "..."]
  // line 1 = seq 70, line 2 = seq 71, line 3 = seq 72, ...
}

// Build completed (success, failure, or revoked).
{
  "type": "build_finished",
  "build_id": 42,
  "status": "success",       // "success" | "failure" | "revoked"
  "error": null               // error message if status == "failure"
}

// Worker is shutting down gracefully.
// worker_id is for logging only — server identifies the connection by its
// internal handle, not this field.
{
  "type": "worker_stopping",
  "worker_id": "worker-arm64-01",
  "reason": "SIGTERM"
}
```

**Tracing:** The `trace_id` from `build_new` is included in all worker-side
log output (via `tracing` spans). This enables correlating server-side and
worker-side log lines for a single build without relying on `build_id` alone.

**Sequence numbers:** The `start_seq` field in `build_output` is a per-build
monotonically increasing counter at **per-line granularity** (starting at 0).
Each line in the batch has an implicit seq: `start_seq`, `start_seq + 1`, etc.
The worker tracks the running total of lines emitted and sets `start_seq`
accordingly. This per-line granularity enables exact SSE resume — a client
that disconnects mid-batch can reconnect with `Last-Event-ID: <line_seq>` and
receive only the lines it hasn't seen, with no duplicates.

### Protocol version negotiation

The `hello` → `welcome` exchange establishes the protocol version. If the
server does not support the worker's protocol version, it responds with an
`error` message (including `min_version` and `max_version` for diagnostics)
and closes the connection.

The server validates the `arch` field against a known enum. Canonical values
are `x86_64` and `aarch64`. The alias `arm64` is accepted as input (mapped to
`aarch64` internally) for compatibility with the existing Python `cbsdcore`
`BuildArch` enum which uses `arm64`. Unknown arch values result in an `error`
message and connection close.

**Arch enum (protocol constant):**

| Canonical | Aliases | Serde |
|-----------|---------|-------|
| `x86_64` | — | `#[serde(rename = "x86_64")]` |
| `aarch64` | `arm64` | `#[serde(rename = "aarch64", alias = "arm64")]` |

Future additions (e.g., `riscv64`) extend this enum.

Note: the previous `worker_register` message has been merged into `hello`.
A single message carries both protocol version and worker capabilities,
eliminating a round-trip and the half-registered worker failure mode.

### Build dispatch flow

```
Server                          Worker
  │                               │
  ├── build_new (JSON) ──────────►│
  ├── <binary: component.tar.gz>─►│
  │                               ├── unpack component
  │                               ├── validate descriptor
  │◄── build_accepted ───────────┤  (or build_rejected → server re-queues)
  │                               ├── launch podman container
  │◄── build_started ────────────┤
  │◄── build_output ─────────────┤  (repeated, batched)
  │◄── build_output ─────────────┤
  │◄── build_finished ───────────┤
  │                               ├── cleanup temp dir
  │                               │
```

### Build revocation flow

```
Server                          Worker
  │                               │
  ├── build_revoke ──────────────►│
  │                               ├── kill podman container
  │◄── build_finished(revoked) ──┤
  │                               │
```

On sending `build_revoke`, the server transitions the build to the
**`REVOKING`** intermediate state. While `REVOKING`:

- The server continues writing `build_output` messages to the log file
  normally (the worker may still be producing output while the container
  shuts down).
- The **revoke acknowledgement timeout** (configurable, default 30 seconds)
  starts. This is separate from and shorter than the liveness grace period —
  a user who calls `DELETE /api/builds/{id}` should not wait two minutes.

On receiving `build_finished(revoked)`: flush logs, set
`build_logs.finished = 1`, transition to `REVOKED`.

If the revoke ack timeout expires without `build_finished`: flush logs, set
`build_logs.finished = 1`, transition to `REVOKED` unilaterally, log a
warning. Any `build_output` arriving after `finished = 1` is **discarded**.
Any late `build_finished` for an already-terminal build is silently discarded.

## Worker Liveness & Reconnection

### Liveness detection

The persistent WebSocket connection is the health signal. No separate heartbeat
mechanism is needed.

```
Worker state machine (server side):

  Connected ──── WS drops ────► Disconnected(since=now)
      │                              │
      │                              ├── reconnects → Connected
      │                              │   (worker sends hello + worker_status
      │                              │    to reconcile; see decision table)
      │                              │
      │                              └── grace_period elapses → Dead
      │                                   → mark active build FAILED("worker lost")
      │                                   → deregister worker
      │
      ├── worker_stopping ──► Stopping
      │                           │
      │                           └── WS drops → Dead (immediate, no grace period)
      │                                → re-queue any DISPATCHED build
      │                                → deregister worker
      │
      └──────── reconnects ──────────── Connected
```

### Grace period

The grace period (configurable, default 90 seconds) accounts for transient
network issues. A WebSocket drop does not immediately mean the worker is dead —
the build may still be running. The server waits for reconnection before
declaring failure.

### Worker reconnect backoff

The worker's reconnect loop uses exponential backoff:

- **Initial interval:** 1 second
- **Multiplier:** 2x
- **Jitter:** ±20%
- **Ceiling:** 30 seconds

**Invariant:** `reconnect_backoff_ceiling_secs` must be **less than**
`liveness_grace_period_secs`. If the ceiling exceeds the grace period, the
worker will be declared dead before it can reconnect — even though it's alive.
The server validates this invariant at startup and refuses to start if
violated. The worker validates its own backoff ceiling against the grace period
value received in the `welcome` message.

### Reconnection protocol

On reconnect, the worker sends `hello` (which includes capabilities), then
optionally `worker_status` if it is currently executing a build. The absence
of `worker_status` after `hello` implies the worker is idle.

### Dispatch acknowledgement timeout

After sending `build_new`, the server expects `build_accepted` or
`build_rejected` within `dispatch_ack_timeout_secs` (configurable, default
15 seconds). If neither arrives in time:

- The build is pushed back to the **front** of its priority lane (re-queued).
- The worker's connection state is marked suspect (but not disconnected).
- Dispatch is retried with the next available worker.

This prevents `DISPATCHED` builds from wedging indefinitely when a worker
accepts the WebSocket message but never responds (e.g., hung process, slow
container runtime startup).

### Reconnection decision table

When a worker reconnects, it sends `hello` (capabilities) and optionally
`worker_status` (if mid-build). The server reconciles based on this table.
**This table is authoritative** — it must be implemented exactly as specified:

| Server state of build N | Worker reports | Server action |
|--------------------------|----------------|---------------|
| `queued` (server crashed between dispatch and DB write) | building N | Send `build_revoke` immediately |
| `dispatched` | building N | Treat as implicit `build_accepted`; transition to `started`; resume log streaming |
| `dispatched` | idle (no `worker_status`) | Re-queue build at front of its priority lane immediately |
| `started` | building N | Resume log streaming; no state change |
| `started` | idle | Mark build `failure` with `error = "worker lost build"` |
| `revoking` | building N | Re-send `build_revoke`; remain in `revoking` |
| `failure` (declared dead) | building N | Send `build_revoke` immediately |
| `revoked` | building N | Send `build_revoke` immediately |
| `success` | building N | Send `build_revoke` immediately |
| Not found (server restarted) | building N | Send `build_revoke` immediately |

### Grace period expiry (no reconnect)

When the grace period elapses without a reconnection, the server applies these
transitions — this is authoritative alongside the reconnection table:

| Server state of build | Transition on grace period expiry |
|-----------------------|-----------------------------------|
| `dispatched` | → `failure` with `error = "worker lost"` |
| `started` | → `failure` with `error = "worker lost"` |
| `revoking` | → `revoked` (unilateral, log warning) |

**Invariant:** If the server receives `worker_status` for a build that is in a
terminal state (`success`, `failure`, `revoked`) or is unknown, the server
sends `build_revoke` immediately. The worker's subsequent `build_finished` for
that build ID is silently discarded.

**Rationale for "always revoke unknown":** An unknown build ID means the server
was restarted or the build was cleaned up. Letting the worker continue produces
a zombie: output streams to nowhere, the build never reaches a terminal state.

### Worker graceful shutdown

On SIGTERM, the worker:

1. Sends `worker_stopping` immediately.
2. Waits up to a configurable timeout for the current build to finish.
3. If the build completes, sends `build_finished` normally.
4. If the timeout expires, kills the container and sends
   `build_finished(revoked)`.
5. Closes the WebSocket connection.

**Server response to `worker_stopping`:** The server transitions the worker to
a `Stopping` state:

- **Remove from dispatch eligibility.** No new builds are sent.
- **Skip grace period on subsequent disconnect.** When a `Stopping` worker's
  connection closes, it is deregistered immediately (not placed in the
  `Disconnected` grace period) because the shutdown was intentional. Any build
  in `DISPATCHED` state assigned to that worker is immediately **re-queued**
  to the front of its priority lane.
- **Do not send `build_revoke`.** Wait for the worker's natural
  `build_finished`. The worker manages its own drain timeout.
- **If `worker_stopping` arrives mid-dispatch** (a `build_new` was just sent
  but `build_accepted` hasn't arrived): the dispatch ack timeout handles it —
  if the worker doesn't accept, the build is re-queued. The ack-timeout path
  must check for `Stopping` state and not mark the worker as "suspect" (the
  shutdown is intentional).

### Server shutdown modes

Two modes, selected by signal:

**Graceful restart (SIGTERM — default for rolling deploys):**

1. Stop accepting new HTTP connections and build submissions.
2. **Do NOT send `build_revoke`.** In-flight builds continue on workers.
3. Flush log data to disk.
4. Close all WebSocket connections.
5. Shut down.

Workers detect the connection drop and enter their reconnection loop.
When the new server instance starts, workers reconnect and the reconnection
decision table handles reconciliation (STARTED + building N → resume).
No builds are terminated. This is the correct behavior for rolling deploys.

**Intentional decommission (SIGQUIT or `--drain` flag):**

1. Stop accepting new HTTP connections and build submissions.
2. Send `build_revoke` to all workers with active builds.
3. Wait up to a configurable drain timeout (default 30 seconds) for workers
   to acknowledge with `build_finished`.
4. For any builds that did not receive acknowledgement, mark them
   `FAILURE` with `error = "server decommissioned"` in the database.
5. Close all WebSocket connections.
6. Flush log data to disk.
7. Shut down.

Use decommission mode when the server is being permanently retired, not
restarted.

## cbscore Integration

The worker calls cbscore (Python) to execute builds. Since cbscore is not being
ported, the Rust worker needs a bridge to Python.

**Recommended approach: subprocess.**

The worker invokes a thin Python wrapper script that:

1. Receives the build descriptor, component path, and `trace_id` via stdin
   as JSON. The wrapper sets `CBS_TRACE_ID=<trace_id>` as an environment
   variable for cbscore's logging layer, enabling cross-boundary correlation.
2. Calls `cbscore.runner.runner()` with the appropriate config.
3. Streams build output (stdout/stderr) back to the Rust worker process.
4. On completion, emits a structured JSON line on stdout (see below).
5. Exits with a classified exit code.

This keeps the boundary clean — no PyO3 embedding, no shared memory, no GIL
concerns. The Rust worker reads the subprocess output line-by-line, batches it,
and sends it to the server over the WebSocket.

### Exit code classification

| Exit code | Meaning | Maps to `build_finished.status` |
|-----------|---------|-------------------------------|
| 0 | Build succeeded | `success` |
| 1 | Build failed (RPM build error, image push rejected, etc.) | `failure` |
| 2 | Infrastructure error (OOM, config error, descriptor malformed) | `failure` |
| 137 (128+9) | Killed by SIGKILL | `revoked` |
| 143 (128+15) | Killed by SIGTERM | `revoked` |

Exit codes 137 and 143 indicate the process was killed by a signal, which
happens when the worker sends SIGTERM during revocation or timeout.

### Structured error output

The Python wrapper emits a final JSON line on stdout before exiting:

```json
{"type": "result", "exit_code": 1, "error": "RPM build failed: spec file not found"}
```

The Rust worker recognizes lines starting with `{"type": "result"` and extracts
the `error` field to populate `build_finished.error`. All other lines are
treated as build output. If the wrapper crashes without emitting a result line,
the worker uses the exit code and stderr as the error message.

### SIGTERM propagation

When `build_revoke` arrives, the worker must terminate the build. The cbscore
subprocess is launched in its own **process group** (`setsid` / new pgid). On
revocation, the worker sends SIGTERM to the entire process group
(`kill(-pgid, SIGTERM)`), which kills both the Python wrapper and any child
processes it spawned (including podman). This prevents orphaned containers.

If the process group does not exit within `sigkill_escalation_timeout_secs`
(configurable, default 15 seconds), the worker escalates to SIGKILL on the
process group.

## BuildDescriptor Struct Layout

The Rust `BuildDescriptor` must preserve the nesting of the Python model for
JSON deserialization compatibility with existing `cbc` clients and historical
`builds.descriptor` blobs. Field names match the Python Pydantic model.

```rust
// In cbsd-proto/src/build.rs

#[derive(Serialize, Deserialize)]
struct BuildDescriptor {
    version: String,
    channel: String,
    version_type: String,   // "release" | "dev" | "test" | "ci"
    signed_off_by: BuildSignedOffBy,
    dst_image: BuildDestImage,
    components: Vec<BuildComponent>,
    build: BuildTarget,
}

#[derive(Serialize, Deserialize)]
struct BuildSignedOffBy {
    user: String,       // display name (overwritten by server)
    email: String,      // email (overwritten by server)
}

#[derive(Serialize, Deserialize)]
struct BuildDestImage {
    name: String,       // full image name incl. registry (e.g., "harbor.clyso.com/ces-devel/ceph")
    tag: String,        // image tag
}

#[derive(Serialize, Deserialize)]
struct BuildComponent {
    name: String,       // component name (e.g., "ceph")
    #[serde(rename = "ref")]
    git_ref: String,    // git ref/branch/tag to build from
    repo: Option<String>, // optional override repo URL
}

#[derive(Serialize, Deserialize)]
struct BuildTarget {
    distro: String,          // e.g., "rockylinux"
    os_version: String,      // e.g., "el9"
    #[serde(default = "default_rpm")]
    artifact_type: String,   // default "rpm"
    arch: Arch,              // x86_64 or aarch64 (arm64 alias)
}

#[derive(Serialize, Deserialize)]
enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64", alias = "arm64")]
    Aarch64,
}
```

**Dispatch reads `descriptor.build.arch`** — the arch field is nested inside
`BuildTarget`, not at the top level. The dispatch logic must access
`queued_build.descriptor.build.arch` when matching against worker capabilities.

**Registry hostname extraction:** For scope checking, the registry hostname is
extracted from `descriptor.dst_image.name` by splitting on the first `/`
(e.g., `harbor.clyso.com/ces-devel/ceph` → `harbor.clyso.com`).

## Server-Side Build Queue

### Priority levels

Builds have one of three priority levels, specified at scheduling time:

| Priority | Value | Default | Use case |
|----------|-------|---------|----------|
| `high`   | 0     | No      | Urgent hotfix builds, release-blocking |
| `normal` | 1     | Yes     | Standard builds |
| `low`    | 2     | No      | Nightly, exploratory, non-urgent |

Priority is an attribute of the build, set at submission time via the REST API
(`POST /api/builds`). If not specified, it defaults to `normal`. Priority
cannot be changed after submission (simplifies queue invariants). Periodic
builds can specify a priority in their task definition.

### Queue design

The server maintains three FIFO lanes, one per priority level. Dispatch always
drains higher-priority lanes first (strict precedence).

```rust
struct BuildQueue {
    high: VecDeque<QueuedBuild>,
    normal: VecDeque<QueuedBuild>,
    low: VecDeque<QueuedBuild>,
    active: HashMap<BuildId, ActiveBuild>,
    workers: HashMap<WorkerId, WorkerState>,
}

// Shared, mutex-protected handle
type SharedBuildQueue = Arc<tokio::sync::Mutex<BuildQueue>>;
```

**Mutual exclusion invariant:** The entire `BuildQueue` is wrapped in a
`tokio::sync::Mutex`. The dispatch sequence is **split** to avoid holding the
lock across I/O:

1. **Under lock:** pop from lane, validate worker availability, insert into
   `active` as `DISPATCHED`, determine target worker and build payload.
   **Write the state transition to SQLite** so the DB reflects `DISPATCHED`
   before the WS send (prevents crash gap). The critical section includes a
   SQLite write (~1–5ms on NVMe-backed WAL) — this is why
   `tokio::sync::Mutex` (async, yield-safe) is required, not `std::sync::Mutex`.
2. **Release lock.**
3. **Outside lock:** Pack component tarball (no caching — re-packed on each
   dispatch; at ~6 KB per component this is negligible). Send `build_new` JSON
   + binary frame over WebSocket.
4. **On send failure:** Re-acquire lock, push build back to front of its lane.

This prevents two concurrent dispatch triggers from popping the same build,
while avoiding blocking all queue operations during a slow WebSocket send.

Selecting the next build to dispatch:

```rust
fn next_pending(&mut self) -> Option<QueuedBuild> {
    self.high.pop_front()
        .or_else(|| self.normal.pop_front())
        .or_else(|| self.low.pop_front())
}
```

On `build_rejected`, the build is pushed back to the **front** of its
respective priority lane (not the back), preserving its position relative to
other builds at the same priority.

### Starvation

With strict precedence, a continuous stream of `high` builds would starve
`normal` and `low` indefinitely. In practice this is unlikely given the current
build volume (single worker, manual submissions, occasional periodic builds).

If starvation becomes a concern in the future, two mitigation options:

1. **Age-based promotion.** A build that has been queued longer than a
   configurable threshold (e.g., 2 hours) is promoted one level up. This
   guarantees eventual execution without complicating the common case.
2. **Weighted round-robin.** Serve N high, then 1 normal, then check high
   again. More complex, less predictable.

Age-based promotion is the recommended future extension. It requires adding a
`queued_at: Instant` field to `QueuedBuild` and checking it during dispatch.
Not needed for v1.

### Dispatch logic

When a build is submitted (via REST API) or a worker becomes idle:

1. Acquire the `BuildQueue` mutex.
2. Call `next_pending()` to get the highest-priority oldest build.
3. Find a connected, idle worker whose `arch` matches the build target.
   **Worker selection strategy (v1):** first idle worker with matching arch.
   This is a valid starting point; revisit when multi-worker deployments are
   operational.
4. Move build to `active` (in `DISPATCHED` state). Record target worker.
   **Write the state transition to SQLite here** (under the mutex) so the DB
   reflects `DISPATCHED` before the WS send. This prevents the crash gap where
   the DB still says `queued` but the build was dispatched in memory.
5. Release the mutex.
6. Pack component tarball. Send `build_new` + binary frame to the worker.
7. Start `dispatch_ack_timeout` timer (default 15 seconds).
8a. On `build_accepted`: **cancel the ack timer**. Build remains in
    `DISPATCHED` state (worker has acknowledged but not yet started).
8b. On `build_started`: transition from `DISPATCHED` to `STARTED`.
9. On `build_rejected`: re-acquire mutex, push build back to front of its
   lane, try the next worker.
10. On send failure: re-acquire mutex, push build back to front of its lane.
11. On ack timeout: re-acquire mutex, push build back to front of its lane.
12. If no workers are available, the build stays in its priority lane.

**DISPATCHED → STARTED gap:** If the worker sends `build_accepted` but
disconnects before `build_started`, the build stays in `DISPATCHED` with no
dedicated timeout. This is intentional — the liveness grace period (60–120s)
handles it. When the worker is declared dead, the reconnection decision table
applies (DISPATCHED + idle → re-queue; DISPATCHED + dead → FAILURE).

**Re-dispatch trigger:** Dispatch runs when (a) a new build is submitted,
(b) a worker sends `build_finished` (becoming idle), (c) a worker reconnects
as idle, or (d) a periodic sweep runs (every 30 seconds) to catch edge cases
like reconnection without build completion.

**Component validation at submission time:** `POST /api/builds` validates the
component name against the server's component store before accepting the build.
Unknown component → 400 Bad Request. This catches errors at submission time
rather than at dispatch time.

**No-matching-worker warning:** When a build is submitted via the REST API
and no connected worker matches the target architecture, the response includes
a warning field: `"warning": "no connected worker matches arch x86_64; build
is queued and will dispatch when a matching worker connects"`. The build is
still accepted (HTTP 202) — this is informational, not an error.

### Build states (server side)

```
QUEUED(priority) → DISPATCHED → STARTED → SUCCESS / FAILURE
                        │           │
                        │           └── REVOKING → REVOKED
                        │
                        └── REJECTED → back to QUEUED(priority)

QUEUED can also transition directly to REVOKED (cancel before dispatch).
DISPATCHED can transition to QUEUED (ack timeout, send failure, worker idle on reconnect).
```

Builds enter `QUEUED` immediately on submission — there is no transient `NEW`
state.

**`DELETE /api/builds/{id}` per-state behavior:**

| Current state | Action | HTTP response |
|---------------|--------|---------------|
| `queued` | Remove from priority lane, mark `REVOKED` synchronously | 200 |
| `dispatched` | Send `build_revoke`, transition to `REVOKING` | 202 |
| `started` | Send `build_revoke`, transition to `REVOKING` | 202 |
| `revoking` | Already revoking, no-op | 200 |
| `success` / `failure` / `revoked` | Already terminal | 409 Conflict |

## Server Startup Recovery

On startup, the server must reconcile the SQLite database with the in-memory
build queue. The database may contain builds in non-terminal states from a
previous server instance.

**Startup procedure:**

1. **Set pragmas.** Set via `SqliteConnectOptions::pragma()` on the sqlx pool
   configuration (per-connection, not per-migration):
   - `PRAGMA journal_mode=WAL;` — concurrent reads with single writer.
   - `PRAGMA foreign_keys=ON;` — **required**. SQLite does not enforce FK
     constraints or `ON DELETE CASCADE` unless this is set on each connection.
     Without it, role/user deletions silently leave orphan rows and the
     last-admin guard produces wrong results.
   - `PRAGMA busy_timeout=5000;` — wait up to 5 seconds on write contention
     (SQLite default is 0ms = fail immediately).
   - `PRAGMA synchronous=NORMAL;` — WAL + NORMAL avoids per-checkpoint fsync
     while maintaining durability against OS crashes. The default FULL is
     unnecessary for this workload.

   **Pool sizing (correctness requirement):** `min_connections = 1,
   max_connections = 4` (or similar). The dispatch mutex is held across a
   SQLite write. If the pool is at capacity, the `sqlx::query!` await stalls
   while holding the mutex. Other queue operations waiting for the mutex that
   also need pool connections would deadlock. Ensuring pool headroom prevents
   this.
2. **Run sqlx migrations.** `sqlx::migrate!("../migrations")` — creates/updates
   all application tables. The `tower_sessions` table is **not** included here;
   it is managed by `tower-sessions-sqlx-store` (see step 3).
3. **Initialize tower-sessions store.** `tower-sessions-sqlx-store` creates its
   own `tower_sessions` table if not present. This is managed by the library,
   not by cbsd migrations. Note: library version upgrades may alter this table.
4. **Fail in-flight builds.** Query for all builds in state `dispatched` or
   `started`. Mark them `failure` with `error = "server restarted"` and set
   `finished_at = now()`. These builds have no active worker connection to
   produce a terminal state.
5. **Re-queue pending builds.** Query for all builds in state `queued`, ordered
   by `queued_at` within each priority level. Insert them into the in-memory
   priority lanes in the correct order.
6. **Ignore terminal builds.** Builds in `success`, `failure`, or `revoked`
   are historical records and require no action.
7. **Begin normal operation.** Start accepting WebSocket connections and HTTP
   requests.

Workers that were connected to the previous server instance will detect the
connection drop and enter their reconnection loop. When they reconnect:
- If idle: they re-register normally.
- If mid-build: they send `worker_status`. The server applies the reconnection
  decision table. Since the server marked that build as `failure` in step 2,
  it will send `build_revoke` (per the "any terminal state" row).

## Build Log Durability

### Write policy

Each `build_output` message received from a worker is **appended to the log
file on disk before the next message is processed**. There is no in-memory
buffering of log data on the server side. This ensures that a server crash
loses at most the single message in flight at the time of the crash.

The `build_logs.log_size` column is updated periodically (every 5 seconds or
on build completion) rather than on every write, to avoid excessive SQLite
write amplification. The `finished` flag is set atomically when
`build_finished` is received.

**Ordering guarantees:** The server processes messages from each worker
connection sequentially (single tokio task per WebSocket connection). There
are no concurrent writes to the same build's log file. No `fsync` is required
per append — the OS page cache is sufficient given the crash-loss window is
one message.

**Late messages:** `build_output` arriving after `build_finished` (possible
due to WebSocket message reordering) is **silently discarded**. The build is
terminal; the log is complete.

### Log file path

Log files are stored at `{log_dir}/builds/{build_id}.log`. This path is
deterministic from the build ID, so the `build_logs.log_path` column stores
the path for auditability but is functionally derivable. The `log_dir` is
configured in the server config.

### Log streaming to clients (SSE)

`GET /api/builds/{id}/logs/follow` uses **Server-Sent Events** (SSE,
`text/event-stream`). SSE is the right choice for browser and CLI log
following: it is unidirectional (server → client), works over standard HTTP,
is HTTP/2 compatible, and has simpler client code than WebSocket.

**Wire format:**

```
event: output
id: 42
data: build output line here

event: output
id: 43
data: another line

event: done
data:
```

- `event: output` — one log line per SSE event.
- `id` — the `seq` number from the `build_output` message. Enables resume.
- `event: done` — sent when `build_logs.finished = 1`. Client can close.

**Resumption:** Client reconnects with `Last-Event-ID: <seq>` header (SSE
built-in). Server replays from the log file starting after that sequence
number.

**Seq-to-offset index:** For active builds, the server maintains an in-memory
`HashMap<BuildId, Vec<(line_seq, file_offset)>>` index. The log writer inserts
one entry per line: `(line_seq, byte_offset_of_line_start)`. Since seq is
per-line granular, SSE resume seeks directly to the exact line — O(log n) via
binary search, not O(n) full file scan. The index is dropped when the build
reaches a terminal state. For completed builds, the server falls back to a
linear scan (acceptable because follow on completed builds is uncommon and the
log is static).

**Notification, not polling:** The log writer publishes notifications via a
`tokio::sync::watch` channel per active build. The `watch` channel is
single-slot and coalesces intermediate updates — it serves as a **wakeup
signal**, not a per-batch delivery mechanism. On each wakeup, the SSE handler
reads from its current file position to EOF, emitting all new lines as SSE
events. This gives sub-millisecond latency vs. 500ms polling.

When the log is complete (`finished = 1`), the server sends the `done` event
and closes the stream.

**Log tail cap:** `GET /api/builds/{id}/logs/tail?n=30` caps `n` at 10000.
Values exceeding the cap return 400 Bad Request. This prevents the server from
reading an entire multi-megabyte log into memory.

### Log file retention

Build logs are retained for `log_retention_days` (configurable, default 30).
A daily periodic task (using `tokio-cron-scheduler` or a simple `tokio::time::interval`):

1. Queries `SELECT build_id, log_path FROM build_logs bl JOIN builds b ON
   bl.build_id = b.id WHERE b.finished_at < unixepoch() - ? AND b.state IN
   ('success', 'failure', 'revoked')`.
2. Deletes the log files from disk.
3. Deletes the `build_logs` rows.
4. Optionally: deletes the `builds` rows (configurable — some deployments want
   permanent build history even without logs).

Build logs for active or queued builds are never GC'd regardless of age.

**GC + SSE race:** Two complementary mitigations:

1. **Hold FD for stream lifetime.** The SSE handler opens the log file **once**
   at stream start and holds the file descriptor for the lifetime of the SSE
   connection. On Linux, an open FD to an unlinked file continues to work (the
   inode lives until all FDs close). This eliminates the race where GC deletes
   the file between reads. This is a **design constraint on `logs/sse.rs`**.

2. **Missing file → synthetic `event: done`.** If the log file does not exist
   at stream start for a terminal build (already GC'd), emit a synthetic
   `event: done` immediately. The client receives a clean termination signal.

## Open Questions

- **Config for cbscore on workers.** Workers still need a local cbscore config
  (`cbscore.config.yaml`) for paths, storage, signing, vault. This config is
  worker-local (filesystem paths, credentials). Should this be managed
  separately, or should parts of it be pushed from the server?
- **Multiple concurrent builds per worker.** Current design is one build at a
  time per worker. Explicitly out of scope for v1. The protocol supports
  extension via `build_id`-namespaced messages, and the server's dispatch logic
  would need to track per-worker build count vs. declared capacity.
- **Build artifact tracking.** Builds produce container images pushed to a
  registry. This is handled by cbscore/podman inside the build container and
  is orthogonal to the task queue design.
- **Periodic builds.** The current system supports cron-scheduled builds.
  The Rust port will need equivalent functionality. This requires a separate
  design document covering scheduling, persistence, and management API.
- **Component versioning and content-addressing.** Currently components are
  filesystem-managed. Future component management API should define whether
  components are versioned and whether there is a content-addressed store.
