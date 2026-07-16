# 012 — Server Event Hook System

## Status

Draft

## Problem

CBS has no push-based notification mechanism for external
systems. Operators who need to trigger CI pipelines on build
completion, post Slack notifications on failures, or update
dashboards when workers go offline must poll the API or parse
logs. There is no way to subscribe to server events.

## Dependencies

This design depends on design 011 (build artifact reporting).
The `build.succeeded` hook payload includes the
`build_report` field, which contains artifact locations and
component metadata. Without 011, the hook can only report
that a build succeeded — not what it produced.

## Design

### Event Taxonomy

Events are grouped by domain. Each has a unique type string
used for hook registration and payload identification.

#### Build Events

| Event | Type | Trigger |
|-------|------|---------|
| Build queued | `build.queued` | User submits build |
| Build started | `build.started` | Worker begins execution |
| Build succeeded | `build.succeeded` | Worker reports success |
| Build failed | `build.failed` | Worker reports failure |
| Build revoked | `build.revoked` | Build cancelled |

`build.dispatched` is excluded — it fires milliseconds
before `build.started` and carries no additional information
that external systems would act on. The dispatch is an
internal scheduling detail; the start is the externally
meaningful event.

**Build event payload:**

```json
{
  "event": "build.succeeded",
  "timestamp": "2026-03-21T04:00:00Z",
  "build_id": 123,
  "trace_id": "a1b2c3d4-...",
  "user_email": "dev@clyso.com",
  "priority": "normal",
  "descriptor": {
    "version": "19.2.3",
    "channel": "ces-devel",
    "components": [...]
  },
  "worker_id": "w-001",
  "worker_name": "host-01-x86",
  "error": null,
  "build_report": { ... }
}
```

The `build_report` field (from design 011) is included only
on `build.succeeded`. It is `null` for all other build
events.

The `descriptor` is included in every build event so that
hook consumers can filter on version, channel, or component
without a follow-up API call.

#### Worker Events

| Event | Type | Trigger |
|-------|------|---------|
| Worker connected | `worker.connected` | WS handshake done |
| Worker disconnected | `worker.disconnected` | WS drops |
| Worker dead | `worker.dead` | Grace period expired |

**Worker event payload:**

```json
{
  "event": "worker.dead",
  "timestamp": "2026-03-21T04:00:00Z",
  "worker_id": "w-001",
  "worker_name": "host-01-x86",
  "arch": "x86_64",
  "reason": "grace period expired",
  "affected_build_id": 123
}
```

`affected_build_id` is non-null when a worker dies with an
in-flight build. This signals that the build will be
re-queued (dispatched state) or failed (started state).

#### Periodic Scheduler Events

| Event | Type | Trigger |
|-------|------|---------|
| Periodic task disabled | `periodic.disabled` | Max retries or fatal error |

Successful periodic triggers produce `build.queued` events.
The only scheduler-specific event worth a hook is when it
gives up — that means expected builds are no longer running.

**Payload:**

```json
{
  "event": "periodic.disabled",
  "timestamp": "2026-03-21T04:00:00Z",
  "task_id": "t-001",
  "summary": "nightly ceph 20.2.0 el9",
  "retry_count": 10,
  "last_error": "unknown component: ceph-nvmeof"
}
```

#### Events Excluded (with rationale)

| Event | Rationale |
|-------|-----------|
| User login/token creation | Audit log concern, not operational. High frequency, low signal for webhooks. |
| User activation/deactivation | Rare admin action; the caller already knows. |
| Worker registered/deregistered | Admin action via REST; caller has the response. |
| Build dispatched | Internal scheduling detail; `build.started` is the external signal. |
| Build output lines | Volume too high for webhooks. Use SSE log follow. |
| HTTP requests | Use the TraceLayer logs. |
| Log GC | Internal housekeeping. |

Any excluded event can be added later by extending the event
enum. The initial set is deliberately minimal.

### Hook Registration

Hooks are managed via `/api/hooks`, restricted to users with
a `hooks:manage` capability.

#### CRUD API

```
POST   /api/hooks          Create hook
GET    /api/hooks          List hooks
GET    /api/hooks/{id}     Get hook (with recent deliveries)
PUT    /api/hooks/{id}     Update (URL, events, enabled)
DELETE /api/hooks/{id}     Delete hook
```

#### Hook Record

```json
{
  "id": "h-001",
  "url": "https://ci.example.com/webhook/cbs",
  "events": ["build.succeeded", "build.failed"],
  "enabled": true,
  "created_by": "admin@clyso.com",
  "created_at": "2026-03-21T04:00:00Z",
  "consecutive_failures": 0
}
```

**`url`** — HTTPS required. HTTP is allowed only for
`localhost` / `127.0.0.1` (development). Validated at
creation time.

**`events`** — Array of event type strings. Wildcards:
`build.*` matches all build events, `*` matches everything.

**`secret`** — HMAC-SHA256 signing key. Generated server-side
at creation time and returned once in the create response.
Not stored in plaintext — the database stores an argon2 hash
for audit, but the raw secret is used at delivery time via
an in-memory cache (populated at server startup and on hook
create/update). The receiver verifies the
`X-CBS-Signature-256` header.

### Delivery

#### Architecture

```
  Event producers          Hook dispatcher
  (route handlers,    ──→  (background task)
   dispatch engine,        │
   scheduler)              ├─ query hooks DB
                           ├─ match event → hooks
                           ├─ POST payload to each URL
                           └─ record delivery result
         │
  mpsc::channel(256)
```

Event producers send `HookEvent` structs to a bounded
`tokio::sync::mpsc` channel. The dispatcher task receives
events and processes them. The channel is bounded (256) to
apply backpressure if the dispatcher falls behind — producers
use `try_send` and log a warning on full.

#### Delivery Protocol

Each delivery is an HTTP POST with:

```
POST <hook.url>
Content-Type: application/json
X-CBS-Event: build.succeeded
X-CBS-Delivery: <uuid>
X-CBS-Signature-256: sha256=<hmac-hex>
```

The HMAC is computed over the raw JSON body using the hook's
secret key (SHA-256).

#### Retry Policy

- 3 attempts with backoff: 10s, 60s, 300s.
- Success: 2xx response within 10 seconds.
- Failure: non-2xx, timeout, or connection error.
- After 3 failures for a delivery, it is marked `failed`.
- After 50 consecutive failed deliveries (across all events
  for a hook), the hook is automatically disabled and a
  `tracing::warn!` is emitted. The admin must re-enable
  manually.

Retries are handled by the dispatcher task using
`tokio::time::sleep` between attempts. No separate retry
queue is needed — the dispatcher processes one event at a
time per hook (serialized delivery order per hook).

#### Delivery Records

```json
{
  "id": "d-001",
  "hook_id": "h-001",
  "event_type": "build.succeeded",
  "status": "delivered",
  "attempts": [
    {
      "at": "2026-03-21T04:00:10Z",
      "http_status": 200,
      "latency_ms": 45
    }
  ],
  "created_at": "2026-03-21T04:00:00Z"
}
```

Delivery records are retained for 7 days (configurable),
then garbage-collected by a background task (similar to the
existing log GC pattern).

### Database Schema

```sql
CREATE TABLE hooks (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    events TEXT NOT NULL,
    secret_hash TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL REFERENCES users(email),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE hook_deliveries (
    id TEXT PRIMARY KEY,
    hook_id TEXT NOT NULL
        REFERENCES hooks(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('pending','delivered','failed')),
    attempts TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_hook_deliveries_hook_id
    ON hook_deliveries(hook_id);
CREATE INDEX idx_hook_deliveries_created_at
    ON hook_deliveries(created_at);
```

**`events`** is stored as a JSON array string (e.g.,
`'["build.succeeded","build.failed"]'`). Matching is done
in application code, not SQL — the hook count is small
(tens, not thousands).

**`secret_hash`** — argon2 hash for audit trail. The raw
secret is cached in memory for HMAC computation.

### Hook Event Production Points

| Event | Code location | Available data |
|-------|---------------|----------------|
| `build.queued` | `routes/builds.rs` after `insert_build_internal()` | build_id, descriptor, user_email, priority |
| `build.started` | `ws/dispatch.rs` `handle_build_started()` | build_id, trace_id, worker_id |
| `build.succeeded` | `ws/dispatch.rs` `handle_build_finished()` (success) | build_id, trace_id, build_report |
| `build.failed` | `ws/dispatch.rs` `handle_build_finished()` (failure) | build_id, trace_id, error |
| `build.revoked` | `ws/dispatch.rs` `handle_build_finished()` (revoked) | build_id, trace_id |
| `worker.connected` | `ws/handler.rs` after hello validation | worker_id, worker_name, arch |
| `worker.disconnected` | `ws/handler.rs` `cleanup_worker()` | worker_id, worker_name |
| `worker.dead` | `ws/handler.rs` `handle_worker_dead()` | worker_id, affected_build_id |
| `periodic.disabled` | `scheduler/mod.rs` max retries block | task_id, summary, error |

At each point, the handler constructs a `HookEvent` and
sends it to the mpsc channel. This is non-blocking —
`try_send` returns immediately.

### Configuration

```yaml
hooks:
  enabled: true
  max-hooks: 20
  delivery-timeout-secs: 10
  retry-attempts: 3
  delivery-retention-days: 7
```

When `hooks.enabled` is `false`, the `/api/hooks` routes
return 404, no dispatcher is spawned, and no events are
produced. This is the default for development deployments.

### Capability

A new capability `hooks:manage` is required for all
`/api/hooks` CRUD operations. The built-in `admin` role
includes it. Non-admin users cannot manage hooks.

## Files Changed

| File | Change |
|------|--------|
| `cbsd-proto/src/lib.rs` | New `HookEvent` enum and payload types |
| `cbsd-server/Cargo.toml` | No new deps (reqwest already present) |
| `cbsd-server/src/config.rs` | Add `HooksConfig` section |
| `cbsd-server/src/app.rs` | Add hook channel to `AppState` |
| `cbsd-server/src/hooks/mod.rs` | New: dispatcher task |
| `cbsd-server/src/hooks/dispatch.rs` | New: delivery logic with retry |
| `cbsd-server/src/hooks/types.rs` | New: event types and payloads |
| `cbsd-server/src/routes/hooks.rs` | New: CRUD API handlers |
| `cbsd-server/src/db/hooks.rs` | New: hook + delivery DB queries |
| `cbsd-server/migrations/` | New: hooks + hook_deliveries tables |
| `cbsd-server/src/routes/builds.rs` | Emit `build.queued` event |
| `cbsd-server/src/ws/dispatch.rs` | Emit build lifecycle events |
| `cbsd-server/src/ws/handler.rs` | Emit worker lifecycle events |
| `cbsd-server/src/scheduler/mod.rs` | Emit `periodic.disabled` |
| `cbsd-server/src/db/seed.rs` | Add `hooks:manage` to admin role |

## Open Questions

1. **Should hook secrets be rotatable?** The current design
   generates the secret once at creation. A
   `POST /api/hooks/{id}/rotate-secret` endpoint could
   generate a new secret, but this adds complexity.

2. **Delivery ordering guarantees.** The dispatcher processes
   events in channel order, but retries for event N could
   delay event N+1 for the same hook. Should retries be
   deferred to a separate retry queue to avoid head-of-line
   blocking?

3. **Bulk event suppression.** During startup recovery, many
   builds may transition to `failure`. Should the hook system
   suppress events during the recovery window to avoid a
   flood of stale notifications?
