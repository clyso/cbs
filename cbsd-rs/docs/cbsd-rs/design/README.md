# cbsd-rs — CBS Daemon Rust Port

Design documentation for reimplementing `cbsd/` (the CBS build service daemon)
in Rust (2024 edition).

## Motivation

The current Python implementation (FastAPI + Celery + Redis) works but has
structural issues:

- Celery is synchronous Python, awkward with async code, and overkill for the
  current single-worker deployment.
- Components live on workers, requiring restart cascades for updates.
- Redis is used solely as Celery plumbing (broker, result backend, log streams)
  and adds operational overhead with no direct value.
- `dbm`-based persistence for users and builds is fragile and hard to query.

The Rust port eliminates Celery and Redis entirely, replaces `dbm` with SQLite,
and introduces a WebSocket-based worker protocol that makes the server the
single source of truth for components.

## Target Architecture

```
┌──────────────────────┐    persistent    ┌────────────────┐
│  cbsd server (Rust)  │◄── WebSocket ───►│  worker (Rust)  │
│                      │   (worker→server │                 │
│  - REST API (axum)   │    initiated)    │  - WS client    │
│  - OAuth/PASETO      │                  │  - cbscore exec │
│  - RBAC permissions  │                  │  - podman build │
│  - build queue       │                  │                 │
│  - build tracker     │                  │                 │
│  - log storage       │                  │                 │
│  - component store   │                  │                 │
│  - periodic tasks    │                  │                 │
└──────────┬───────────┘                  └────────────────┘
           │
     ┌─────┴─────┐
     │  cbsd.db  │  SQLite (single file, WAL mode)
     │  (sqlx)   │  users, tokens, roles, builds, logs, periodic tasks
     └───────────┘
```

Key properties:

- **No Redis.** Server owns queue, tracker, and log storage directly.
- **No Celery.** Single persistent WebSocket per worker replaces the entire
  broker/result/event infrastructure.
- **Server owns components.** Workers receive component tarballs per build.
- **Workers are pure clients.** Outbound connection only — no listening port,
  no TLS cert management on worker side.
- **Single SQLite database.** All persistent state in one file.

## Technology Decisions

| Concern | Choice | Crate(s) | Rationale |
|---------|--------|----------|-----------|
| HTTP framework | axum | `axum`, `tower`, `tokio` | Mature, composable, built-in WebSocket |
| TLS | rustls | `axum-server`, `rustls` | Pure Rust, no OpenSSL dependency |
| OAuth 2.0 (Google SSO) | Manual OIDC flow | `reqwest` | ~200 LoC; no mature axum OAuth server crate |
| OAuth session state | tower-sessions | `tower-sessions` | Ephemeral, only for OAuth CSRF state |
| Token format | PASETO v4 | `pasetors` | Wire-compatible with current Python tokens |
| Auth extraction | axum custom extractors | `axum-extra` (TypedHeader) | Idiomatic; `AuthUser` and `RequireCap<C>` |
| Permission model | RBAC, string-enum caps | — (custom) | Caps stored in SQLite, dynamic via API |
| Scope matching | Glob patterns | `glob-match` or equivalent | Replaces error-prone regex patterns |
| Database | SQLite (WAL) | `sqlx` | Single file, compile-time checked queries, async |
| Serialization | serde | `serde`, `serde_json`, `serde_yml` | JSON for API/WS, YAML for server config (`serde_yml` — actively maintained fork of `serde_yaml`) |
| API types (shared) | serde structs | `serde`, `chrono` | Port of cbsdcore Pydantic models |
| Logging | tracing | `tracing`, `tracing-subscriber`, `tracing-appender` | Structured, async-aware |
| Cron scheduling | cron crate | `cron`, `tokio-cron-scheduler` | Periodic builds (design TBD) |
| Worker WS protocol | JSON + binary frames | `serde_json`, axum WS | JSON for messages, binary for component tarballs |
| cbscore bridge | Subprocess | `tokio::process` | Python wrapper script, no PyO3/GIL |
| API key hashing | argon2 | `argon2` | Offline brute-force resistance for API keys |
| Token hashing | SHA-256 | `sha2` | Fast per-request revocation check for PASETO tokens |
| Rate limiting | tower-governor | `tower-governor` | Token-bucket per-IP rate limiting on auth endpoints |

## Design Documents

| Document | Scope |
|----------|-------|
| [Architecture & Task Queue](2026-03-13-cbsd-rust-port-design.md) | Server/worker architecture, WebSocket protocol (trace_id, per-line seq, SHA-256 integrity, arch validation, Bearer auth on upgrade), component distribution, build queue (priority lanes, split-mutex dispatch, dispatch ack timeout with lifecycle), worker liveness & reconnection (full decision table), revoking intermediate state, server startup recovery (WAL + busy_timeout), graceful shutdown (server + worker + Stopping state), cbscore subprocess bridge (classified exit codes, process-group SIGTERM, structured error), log durability (per-line seq→offset index, watch channel wakeup, SSE streaming, tail cap) |
| [Authentication, Permissions & User-Facing API](2026-03-13-cbsd-auth-permissions-design.md) | Google SSO OAuth flow (SQLite-backed sessions, client_type propagation, domain restriction), PASETO tokens (SHA-256, max_token_ttl), API keys (argon2 + LRU verification cache, 12-char random prefix, self-service), RBAC with per-assignment scopes (user_role_scopes table, scope-dependent validation), endpoint gating (RequireCap + :own/:any ownership enforcement), SQLite schema (auth + builds + descriptor_version), whoami response spec, signed_off_by server override, token/key revocation (self + bulk), first-startup bootstrapping sequence, last-admin guard (5 mutation paths, builtin cap protection), user deactivation (bulk-revoke), migration plan (tokens, API paths, breaking changes, release ordering) |

| [Project Structure & Crate Organization](2026-03-14-cbsd-project-structure.md) | Cargo workspace layout (3 crates: cbsd-proto, cbsd-server, cbsd-worker), dependency profiles, sqlx migration mechanism |
| [Worker Registration](2026-03-16-worker-registration.md) | Worker WebSocket registration, server-assigned connection handles, heartbeat protocol |
| [Config Keys: kebab-case](2026-03-17-kebab-case-config.md) | YAML config keys use kebab-case, serde `rename_all` at deserialization boundary |
| [Compile-Time Checked SQL Queries](2026-03-17-sqlx-compile-time-queries.md) | Migration from `sqlx::query("...")` to `sqlx::query!("...")` macros, `.sqlx/` offline cache |
| [cbscore Wrapper](2026-03-18-cbscore-wrapper.md) | Python subprocess bridge for cbscore, process-group management |
| [Periodic Builds](2026-03-18-periodic-builds.md) | Cron scheduling, tag interpolation, retry logic, periodic task REST API |
| [Dev Mode OAuth Bypass](2026-03-20-dev-oauth-bypass.md) | Skip Google OAuth round-trip in dev mode using `seed.seed-admin` as email identity, ~40 LoC server change |

### Planned documents

None — all identified topics have design documents.

## REST API Surface

All endpoints are prefixed with `/api`. Authentication is via
`Authorization: Bearer <token>` header unless noted otherwise.

### Authentication

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| GET | `/auth/login?client=cli\|web` | None | — | Redirect to Google SSO |
| GET | `/auth/callback` | None | — | OAuth callback, creates user + token |
| GET | `/auth/whoami` | Required | — | Current user info (email, name, roles, scopes, effective caps) |
| POST | `/auth/token/revoke` | Required | — | Revoke current bearer token (self-revocation, no request body) |
| POST | `/auth/tokens/revoke-all` | Required | `permissions:manage` | Revoke all tokens for a given user email |

**Rate limiting:** `/auth/login` and `/auth/callback` are rate-limited to
10 requests/minute per IP via `tower-governor`. This is a default; operators
can adjust via server config.

### API Key Management

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| POST | `/auth/api-keys` | Required | `apikeys:create:own` or `permissions:manage` | Create API key (plaintext returned once) |
| GET | `/auth/api-keys` | Required | — | List own API keys (prefix + metadata) |
| DELETE | `/auth/api-keys/{prefix}` | Required | — | Revoke API key by prefix (own, or any with `permissions:manage`) |

### Permissions — Roles

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| GET | `/permissions/roles` | Required | `permissions:view` | List all roles |
| POST | `/permissions/roles` | Required | `permissions:manage` | Create custom role |
| GET | `/permissions/roles/{name}` | Required | `permissions:view` | Role details (caps + scopes) |
| PUT | `/permissions/roles/{name}` | Required | `permissions:manage` | Update role caps/scopes |
| DELETE | `/permissions/roles/{name}` | Required | `permissions:manage` | Delete role (fails for builtins) |

### Permissions — User Assignments

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| GET | `/permissions/users` | Required | `permissions:view` | List users + their roles |
| GET | `/permissions/users/{email}/roles` | Required | `permissions:view` | Roles for a specific user |
| PUT | `/permissions/users/{email}/roles` | Required | `permissions:manage` | Set roles (replace all) |
| POST | `/permissions/users/{email}/roles` | Required | `permissions:manage` | Add a role |
| DELETE | `/permissions/users/{email}/roles/{role}` | Required | `permissions:manage` | Remove a role |

### Builds

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| POST | `/builds` | Required | `builds:create` + scope (channel/registry/repo) | Submit a new build (optional `priority`). Returns 202; includes `warning` if no matching worker connected. |
| GET | `/builds` | Required | `builds:list:own` or `builds:list:any` | List builds (filterable by state, user) |
| GET | `/builds/{id}` | Required | `builds:list:own` or `builds:list:any` | Single build status + details |
| DELETE | `/builds/{id}` | Required | `builds:revoke:own` or `builds:revoke:any` | Revoke/cancel a build (200 if queued/sync, 202 if active/async, 409 if terminal) |
| GET | `/builds/{id}/logs/tail?n=30` | Required | `builds:list:own` or `builds:list:any` | Last N log lines |
| GET | `/builds/{id}/logs/follow` | Required | `builds:list:own` or `builds:list:any` | Stream logs (SSE, `text/event-stream`) |
| GET | `/builds/{id}/logs` | Required | `builds:list:own` or `builds:list:any` | Full log file download |

**Scope filtering on GET `/builds`:** A user with only `builds:list:own` sees
only their own builds — the server implicitly filters by the requesting user's
email. A user with `builds:list:any` can filter by any user via query parameter.
The `?user=` filter is rejected with 403 if the caller lacks `builds:list:any`.

### Components

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| GET | `/components` | Required | — | List available components + versions. No capability required beyond authentication — component names and versions are not sensitive. |

Future (component management through API):

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| POST | `/components` | Required | `components:manage` | Add/update a component |
| DELETE | `/components/{name}` | Required | `components:manage` | Remove a component |

### Workers

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| GET | `/workers` | Required | `workers:view` | List connected workers + status |
| WS | `/ws/worker` | API key | — | Worker WebSocket endpoint (see protocol doc) |

### Admin

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| GET | `/admin/queue` | Required | `admin:queue:view` | Queue state, active builds, worker assignments. Admin-only, intentionally unscoped. |
| PUT | `/admin/users/{email}/deactivate` | Required | `permissions:manage` | Deactivate user (bulk-revokes all tokens + API keys) |
| PUT | `/admin/users/{email}/activate` | Required | `permissions:manage` | Reactivate user (does not restore revoked credentials) |

Note: `/admin/` prefix avoids axum routing conflicts and signals admin-only
semantics. `admin:queue:view` returns the full global queue for debugging —
only the `admin` role should be granted this capability.

### Periodic Builds

| Method | Path | Auth | Cap | Description |
|--------|------|------|-----|-------------|
| POST | `/periodic` | Required | `periodic:create` + `builds:create` (scoped) | Create periodic task |
| GET | `/periodic` | Required | `periodic:view` | List periodic tasks |
| GET | `/periodic/{id}` | Required | `periodic:view` | Get periodic task details |
| PUT | `/periodic/{id}` | Required | `periodic:manage` (+ `builds:create` if descriptor changed) | Update periodic task |
| DELETE | `/periodic/{id}` | Required | `periodic:manage` | Delete periodic task |
| PUT | `/periodic/{id}/enable` | Required | `periodic:manage` | Enable periodic task |
| PUT | `/periodic/{id}/disable` | Required | `periodic:manage` | Disable periodic task |

## Capabilities Reference (v1)

| Capability | Description | Scope-gated |
|-----------|-------------|-------------|
| `builds:create` | Submit new builds | Yes (channel/registry/repository) |
| `builds:revoke:own` | Cancel own builds | No |
| `builds:revoke:any` | Cancel any user's builds | No |
| `builds:list:own` | View own builds and their logs | No |
| `builds:list:any` | View all builds and their logs | No |
| `admin:queue:view` | View queue internals, worker assignments (admin-only, unscoped) | No |
| `permissions:view` | View roles, user assignments | No |
| `permissions:manage` | Create/modify/delete roles, assign roles to users, manage any API key, bulk-revoke tokens | No |
| `apikeys:create:own` | Create and manage own API keys (self-service) | No |
| `components:manage` | Add/update/remove component definitions | No |
| `workers:view` | View connected workers and their status | No |
| `*` | All capabilities (admin wildcard) | No |

Capability strings are validated at the API layer. Unknown strings are rejected
with 400 to prevent silent typos.

| `periodic:create` | Create periodic build tasks | No |
| `periodic:view` | View periodic build tasks | No |
| `periodic:manage` | Update, delete, enable, disable periodic tasks | No |

## Decided Questions

Previously open, now resolved:

- **Log streaming for clients.** SSE (`text/event-stream`) with `watch`-channel
  notifications (not polling). `Last-Event-ID` header + seq→offset index for
  O(1) resumption. See arch doc.
- **Multiple concurrent builds per worker.** Explicitly out of scope for v1.
- **Token migration at cutover.** Accept the break; users re-authenticate once.
- **API path compatibility.** Coordinated release of cbsdcore + cbc + server.
- **Scope model.** Per-assignment scopes (not per-role). See auth doc.
- **Worker identity.** Server-assigned connection handle (UUID), not
  `worker_id` string. `worker_id` is display-only.
- **Dispatch ack timeout.** 15 seconds default. Build re-queued on expiry.
- **Revoking state.** `REVOKING` intermediate state between `STARTED` and
  `REVOKED`, with 30-second ack timeout.
- **Periodic builds.** Deferred to post-v1. No endpoint stubs in v1.
- **Component tarball caching.** No caching in v1 (re-pack on each dispatch).
  ~6 KB per component, negligible overhead.
- **Queue inspection scope.** `admin:queue:view` is admin-only, intentionally
  unscoped. Returns full global queue.
- **Token hash spec.** SHA-256 of the raw UTF-8 PASETO token string.
- **`signed_off_by` handling.** Server overwrites from `users` table.
- **Re-dispatch trigger.** On build submission, worker idle, worker reconnect,
  or 30-second periodic sweep.
- **`build_output` after `build_finished`.** Silently discarded.
- **Startup recovery DB failure.** Abort startup. Partial state is dangerous.
- **Rate limiting.** `tower-governor`, 10 req/min per IP on auth endpoints.
- **Session store.** SQLite-backed `tower-sessions` (survives restarts).
- **Bootstrapping sequence.** Seeded in single transaction: roles → admin
  user → admin role assignment → worker API keys. See auth doc.
- **SSE per-line seq.** `build_output` carries `start_seq`; per-line seq =
  `start_seq + index`. Each SSE event has exact per-line ID. No duplicates
  on mid-batch reconnect.
- **Google domain restriction.** `allowed_domains` config, server-side check
  at callback, `hd=` hint on auth URL.
- **API key verification cache.** In-memory LRU (512 entries), keyed by
  SHA-256 of raw key. Purged on revocation.
- **Last-admin guard scope.** Checked on all 5 mutation paths (role
  assignment PUT/DELETE, user deactivation, role deletion, role cap update).
  Builtin roles cannot have caps modified.
- **:own/:any enforcement.** Handler loads resource, checks ownership if
  caller has only `:own`. Documented as a standard pattern.
- **Worker auth transport.** `Authorization: Bearer` header on WS upgrade.
  Never in query string.
- **Dispatch ack timer lifecycle.** `build_accepted` cancels timer.
  DISPATCHED→STARTED gap covered by liveness detection.
- **User deactivation.** Bulk-revokes all tokens and API keys. Reactivation
  does not restore revoked credentials.
- **`max_token_ttl_seconds`.** Added to config, default 0 (infinite).
- **Token revocation body.** Self-revocation: infer from bearer (no body).
  Bulk: `POST /tokens/revoke-all` with `user_email`.
- **Role deletion with assignments.** 409 if assignments exist; `?force=true`
  to CASCADE (still subject to last-admin guard).
- **sqlx migrations.** `sqlx::migrate!()` at startup with embedded `.sql`
  files. Backward-incompatible schema changes require coordinated deploys.
- **Project structure.** Cargo workspace with 3 crates: `cbsd-proto`,
  `cbsd-server`, `cbsd-worker`. See project structure doc.
- **Scope type naming.** `channel` (not `project`) — matches existing
  `BuildDescriptor.channel` field. Scope types: `channel`, `registry`,
  `repository`.
- **Arch enum.** Canonical `aarch64`, alias `arm64` accepted via serde. Added
  to migration breaking changes.
- **SQLite pragmas.** `foreign_keys=ON` (required for FK/CASCADE), WAL,
  `busy_timeout=5000`, `synchronous=NORMAL`.
- **`max_token_ttl_seconds`.** Config value `none` = no limit (default).
  Positive integer = clamp tokens to that TTL.
- **Worker reconnect backoff.** 1s initial, 2x multiplier, ±20% jitter, 30s
  ceiling. Invariant: ceiling < grace period. Validated at startup.
- **Grace period expiry transitions.** Specified as authoritative table
  alongside reconnection decision table.
- **Dispatch DB write ordering.** SQLite write to `dispatched` happens under
  mutex before WS send. Prevents crash gap.
- **Component integrity failure.** Marks build `failure` (not re-queue) —
  server-side corruption would fail on all workers.
- **Scope evaluation semantics.** Assignment-level AND — all scope checks for
  a build must be satisfied by the same assignment.
- **Repository scope source.** `descriptor.components[].repo` overrides only.
- **Registry scope in builds.** Checks `dst_image.name` hostname at
  submission.
- **PASETO payload schema.** Frozen as `CBSD_TOKEN_PAYLOAD_V1`.
- **API key LRU cache invalidation.** Reverse indices (by_prefix, by_owner).
- **Web UI auth.** Bearer in localStorage. No session-cookie API auth.
- **Session fixation prevention.** Session ID regenerated at callback.
- **OAuth session TTL.** 10 minutes.
- **`descriptor_version`.** V1 = current shape. Unknown → error.
- **Log file retention.** `log_retention_days: 30`, daily GC.
- **Stopping worker + DISPATCHED.** Re-queued on disconnect.
- **Build ID continuity.** Autoincrement from MAX(existing) + 1 on migration.
- **PASETO `expires` format.** Unix epoch integer (i64), not ISO 8601. Keys
  alphabetically ordered. Pinned canonical JSON form with CI cross-language
  hash verification test.
- **Server shutdown modes.** SIGTERM = graceful restart (don't revoke, workers
  reconnect). SIGQUIT/`--drain` = decommission (revoke + wait).
- **ApiKeyCache mutex.** `Arc<Mutex<ApiKeyCache>>` with LRU eviction cleanup
  of reverse maps.
- **BuildDescriptor struct layout.** Preserves Python nesting: `arch` at
  `descriptor.build.arch`. Full Rust struct in arch doc.
- **Worker TLS.** `tls_ca_bundle_path` config for private CAs.
- **Log GC + SSE race.** Synthetic `event: done` if log file missing for
  terminal build.
- **`build_revoke` before `build_accepted`.** Worker responds with
  `build_finished(revoked)` immediately.
- **`trace_id` to cbscore.** Included in subprocess stdin JSON, set as
  `CBS_TRACE_ID` env var.
- **PASETO ISO 8601 vs epoch divergence.** Documented: Python uses ISO 8601,
  Rust uses epoch. Not hash-compatible. Zero-downtime token migration not
  supported for v1.
- **BuildDescriptor completeness.** Added `version_type` and `artifact_type`
  (missing from initial Rust struct layout).
- **ApiKeyCache eviction.** `CachedApiKey` includes `key_prefix`. Concrete
  `push()` + reverse-map cleanup pattern specified.
- **`GET /builds/{id}` response shape.** JSON example added to auth doc.
  Field name changes added to breaking changes list.
- **Error response schema.** `{"detail": "..."}` — same as Python/FastAPI.
- **Activate/deactivate idempotency.** Early return on no-op, no guard/revoke.
- **SQLite pool sizing.** `max_connections = 4` as correctness requirement
  (prevents deadlock with dispatch mutex).
- **SSE FD lifetime.** Handler opens file once, holds FD for stream duration.
  Design constraint on `logs/sse.rs`.

## Open Questions

Consolidated from all design documents:

- **cbscore config on workers.** Workers need local cbscore config for paths,
  storage, signing, vault. Managed separately or partially pushed from server?
- **Build artifact tracking.** Images are pushed to registries by cbscore/podman
  inside the build container. Should the server track which images were produced?
  Relevant for auditing and GC.
- **Component management API.** Currently components are filesystem-managed on
  the server. Future work to enable CRUD via REST API and web UI. Needs schema
  decisions: are components versioned? Is there a content-addressed store?
- **User account lifecycle.** Users can be deactivated by admins (see auth doc).
  Should deactivated accounts be auto-cleaned after a retention period?
- **Worker container image specifics.** Python environment version,
  uv/venv/system packages, how `cbscore.config.yaml` is injected — all
  unspecified. Near-term implementation blocker.
- **`glob-match` wildcard semantics.** Verify that `*` matches path
  separators in `glob-match`. If it does, `ces-devel/*` matches
  `ces-devel/subdir/repo` — may not be intended. Verify at implementation.
