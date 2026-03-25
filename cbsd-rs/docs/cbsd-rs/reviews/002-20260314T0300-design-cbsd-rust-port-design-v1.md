# Design Review: cbsd Rust Port — Architecture, Auth/Permissions & API Surface

**Documents reviewed:**


- `cbsd-rs/docs/cbsd-rs/design/README.md`
- `cbsd-rs/docs/cbsd-rs/design/002-20260313T1800-cbsd-rust-port-design.md`
- `cbsd-rs/docs/cbsd-rs/design/003-20260313T2129-cbsd-auth-permissions-design.md`

---

## Summary

The overall architectural direction is correct and the simplification is well-motivated: eliminating Redis and Celery, making the server the component authority, and replacing the brittle YAML permissions with RBAC are all the right calls. The WebSocket protocol is well-specified, the technology choices are defensible, and several individual decisions (argon2/SHA-256 dual hashing, process-group SIGTERM propagation, two-phase extractor+handler auth) are genuinely strong.

The design is **not ready to implement as-written**. There are 8 blockers — none requiring architectural rethink, all with clear resolution paths. The most critical is B1: the `user_roles` schema has no mechanism for per-user scope assignment, making the "one builder role, scoped differently per user" use case architecturally impossible without schema changes.

**Verdict: Revise and re-review.**

---

## Blockers

Issues that must be resolved before any implementation begins.

### B1 — Scope enforcement is impossible with the current schema

The permission model's central use case is assigning the `builder` role to different users with different scopes (e.g., user A scoped to `harbor.clyso.com/ces-devel/*`, user B to `ces-prod/*`). But scopes live in `role_scopes`, attached to the role definition, not to individual user-role assignments. The `user_roles` junction table has only `(user_email, role_name)` — no scope column.

This means assigning different scopes requires creating separate roles (`builder-ces-devel`, `builder-ces-prod`, etc.), collapsing back to the complexity the design claims to eliminate. The default `builder` role says "(must be scoped per assignment)" but the schema provides no mechanism for per-assignment scopes.

**Fix:** Either add a `user_role_scopes` table for per-assignment scope overrides (Option A), or commit explicitly to one-role-per-scope and remove the per-assignment language (Option B). The current text claims A but implements B.

### B2 — Dispatch ack timeout is unspecified — DISPATCHED builds can wedge indefinitely

After sending `build_new`, the server waits for `build_accepted` or `build_rejected`. The only timeout is the liveness grace period (60–120s). If the connection drops after `build_new` is sent but before `build_accepted` arrives, and the worker restarts and reconnects idle, the build sits in DISPATCHED until the grace period expires — then is marked FAILURE, not re-queued. The build is permanently lost.

The reconnection decision table also has a gap: no row for "server build = DISPATCHED, worker reconnects idle."

**Fix:** Add a `dispatch_ack_timeout_secs` config (10–15s). On expiry without ack, re-queue the build at the front of its lane. Add the missing reconnection table row: DISPATCHED + worker idle → re-queue immediately.

### B3 — `build_output` after `build_revoke` can write to a closed log

The 30s revoke ack timeout expires, the server marks the build REVOKED and sets `build_logs.finished = 1`. But `build_output` messages can still arrive — the design doesn't specify what happens. The SSE `follow` endpoint uses `finished = 1` as the signal to send `done` and close the stream. Late log lines written after `finished = 1` are persisted but never surfaced to clients.

**Fix:** Introduce a `Revoking` intermediate state. While `Revoking`, continue writing logs normally. On timeout expiry, flush, transition to REVOKED, then set `finished = 1`. Discard any `build_output` arriving after `finished = 1`.

### B4 — `BuildDescriptor.signed_off_by` has no source in the new auth model

The `BuildDescriptor` contains a mandatory `signed_off_by` field with `user` (full name) and `email`. The current `cbc` constructs this from the `UserConfig` JSON downloaded at OAuth time. The new auth flow returns only a PASETO token — `cbc` has no source for the display name. `GET /api/auth/whoami` could provide it, but its response shape is unspecified.

**Fix:** The Rust server should ignore the client-submitted `signed_off_by` and overwrite it from the authenticated user's `users` table record. Specify the `whoami` response shape (email, name, roles, effective caps) so `cbc` can cache the name locally after auth.

### B5 — `worker_id` creates a split-identity scenario on reconnect

The `workers` in-memory map is keyed by `worker_id` (a free-form string from `hello`). Nothing prevents two different API keys from advertising the same `worker_id`, overwriting each other's `WorkerState`. During the grace period, a new connection with the same `worker_id` has undefined behavior — is it a reconnect or a new registration?

**Fix:** Key the `workers` map by a server-assigned opaque connection handle, not by `worker_id` string. Use `worker_id` only as a human-readable display label. Reconciliation on reconnect matches by `build_id`, not by `worker_id`. Remove `worker_id` from `worker_status` (redundant — the connection is already identified).

### B6 — `api_keys.owner_email` FK blocks worker bootstrapping on a fresh system

`api_keys.owner_email` is `NOT NULL REFERENCES users(email)`. On first deployment, `users` is empty. The `seed_worker_api_keys` config fires, hits a FK violation, and the server cannot start. The seeding order (`seed_admin` first → then API keys) is not specified, and it doesn't cover worker keys not owned by the admin.

**Fix:** Either make `owner_email` nullable (`NULL` = system/infrastructure key), or create a synthetic `system` user at DB-init time. Specify the exact seeding order in the design document.

### B7 — Periodic builds: schema absent, feature contract unspecified

The design declares 5 REST endpoints and 2 capabilities for periodic builds, but provides no SQLite schema, no request/response body specification, and no behavioral contract. The existing Python implementation has significant semantics not acknowledged: `tag_format` substitution with 6 variables, exponential backoff (30s base, 1.5x factor, 10m cap), `enabled`/auto-disable state machine, and different URL paths than the design proposes.

**Fix:** Either (a) explicitly remove periodic builds from v1 scope and delete the endpoint stubs from the README, or (b) write the prerequisite design document before implementation starts. What is unacceptable is 5 declared endpoints with capabilities and no backing design.

### B8 — `build_logs.log_size` is not a reliable SSE resume cursor

The SSE `follow` endpoint resumes via `Last-Event-ID: <seq>`, but the log file contains raw text lines, not seq-indexed records. `log_size` is updated every 5 seconds, not per-line. There is no mechanism to map a seq number to a file offset. Resuming from seq N requires scanning the entire log file from the beginning — O(n) per reconnect, pathologically expensive under frequent reconnects.

**Fix:** Maintain an in-memory `seq → file_offset` index for active builds. The log writer inserts `(seq, offset_after_write)` on each batch append. SSE resume seeks directly to the offset. Drop the index when the build reaches a terminal state.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — Mutex held across WebSocket send blocks all queue operations

The dispatch logic holds the `BuildQueue` mutex while sending the `build_new` JSON + binary frames. A WebSocket send involves kernel I/O — if the worker is slow to drain its receive buffer, all queue operations (submissions, dispatches, revocations) stall.

**Fix:** Split dispatch: within the mutex, pop + mark DISPATCHED (pure memory, sub-microsecond). Release mutex. Send WS message. On send failure, re-acquire mutex and push the build back to front of its lane.

### M2 — `tower-sessions` in-memory store will abort OAuth flows on server restart

The default tower-sessions store is in-memory. A server restart (deploy, crash, OOM) clears all in-flight OAuth sessions — any user mid-flow gets a CSRF validation failure. The session signing key source is also unspecified.

**Fix:** Use SQLite as the session backing store. Sessions are short-lived and rare. Specify the signing key derivation source explicitly.

### M3 — `?client=cli|web` propagation across OAuth round-trip is unspecified

The `?client=` parameter is on `/api/auth/login`, but Google's callback only carries `code` and `state`. At callback time the server doesn't know whether to render a paste-page or redirect to localhost.

**Fix:** Store `client_type` in the tower-sessions session at login time, read it at callback time. Document this explicitly.

### M4 — Component tarball caching has no invalidation strategy

The server packs a component tarball on each dispatch. If cached without detecting filesystem changes, stale tarballs will be dispatched after component updates. `component_sha256` checks transfer integrity, not content freshness.

**Fix:** Cache keyed by `(component_name, hash_of_directory_contents)`. Use filesystem watch notifications or mtime-based invalidation.

### M5 — `DELETE /api/auth/api-keys/{prefix}` is unsafe without uniqueness guarantee

8 hex characters = 32 bits of space. No `UNIQUE` constraint on `(owner_email, key_prefix)`. Admin deletions could hit the wrong key if two keys share a prefix.

**Fix:** Add `UNIQUE(owner_email, key_prefix)`. For admin deletions, require both `owner_email` and `key_prefix`, or expose the integer `id` in the list response.

### M6 — `builds.descriptor` JSON blob has no versioning strategy

As the Rust `BuildDescriptor` evolves, old rows will contain JSON that no longer deserializes correctly. No schema version on the blob.

**Fix:** Add `descriptor_version INTEGER NOT NULL DEFAULT 1` to `builds`. On schema changes, a sqlx migration transforms existing blobs and bumps the default.

### M7 — `worker_stopping` server response is unspecified

The design says the server "stops dispatching" but doesn't specify: Does it mark the worker `Stopping`? Does the grace period apply after an intentional shutdown? Does it send `build_revoke` or wait passively? What if `worker_stopping` arrives mid-dispatch?

**Fix:** Add a `Stopping` state to `WorkerState`. On receipt: remove from dispatch eligibility, skip grace period on subsequent disconnect, do not send `build_revoke` — wait for the worker's natural `build_finished`.

### M8 — `periodic:create` as required cap for list endpoints is semantically wrong

`GET /periodic/builds` requires `periodic:create or periodic:manage`. This conflates write and read access — you cannot grant view-only access to the periodic schedule without also granting creation ability. Inconsistent with the `builds:list:own`/`builds:list:any` pattern.

**Fix:** Add a `periodic:view` capability for read-only access.

### M9 — `builds:inspect` endpoint has no scope — full queue dump exposed

`GET /builds/inspect` requires `builds:inspect` but returns the entire global queue with all user submissions and descriptors. A user with `builds:inspect` but not `builds:list:any` sees more than intended — capability escalation.

**Fix:** Either document explicitly that `builds:inspect` is admin-only and intentionally bypasses scopes, or scope the queue dump to the caller's effective `builds:list` scope.

---

## Minor Issues

- **`arch` field is free-form.** `hello.arch` is a string; `BuildArch` in cbsdcore is an enum with `"arm64"` and `"x86_64"`. A worker advertising `"aarch64"` silently never matches. Enumerate allowed values in the protocol and reject unknown values at registration time.
- **Protocol version mismatch response** should include the server's supported range, not just "unsupported."
- **API key prefix length** (8 chars) gives poor collision resistance. Specify 12–16 chars explicitly.
- **Worker `worker_id` stability** across restarts should be documented as a known behavior (new ID = new registration, old entry times out).
- **Infinite TTL tokens** have no bulk-revoke mechanism after a suspected compromise. Note this as a gap.
- **`log_path` in `build_logs`** is described as redundant (deterministic from build ID). Either drop the column or justify storage.
- **`DELETE /api/builds/{id}`** for cancellation retains the record — document the intentional non-idempotency (second DELETE → 409 Conflict).
- **`DELETE /api/builds/{id}` for QUEUED state** is not covered in the revocation flow. Specify: remove from priority lane, mark REVOKED synchronously, no WS message needed.
- **SIGKILL escalation timeout** has no named config key. Name it consistently with the other timeout configs.
- **No rate limiting** on `/api/auth/login` and `/api/auth/callback`. Note as deployment-time operator responsibility.
- **`role_caps` stores capability strings with no validation.** Typos silently never match. Validate at the API layer, return 400 for unknown capability strings.
- **`role_scopes` has no `UNIQUE` constraint** on `(role_name, scope_type, pattern)`. Duplicate entries are harmless but confusing. Add the constraint.
- **SSE log streaming is a breaking change for cbc.** Current `cbc build logs follow` uses polled JSON; the new server returns `text/event-stream`. Add to coordinated release notes.
- **`builds.user_email` and `signed_off_by` in the descriptor blob will diverge** if a user's Google display name changes. Specify which source `GET /builds/{id}` returns.
- **Component validation at build submission time** is not specified. The Python server validates component names before accepting builds. Without this, unknown component errors surface at dispatch time, not submission time.
- **`NewBuildResponse.state` changes** from `"PENDING"` (uppercase, Celery) to `"queued"` (lowercase). Note as user-visible change in migration plan.
- **HTTP status code** for revoke of QUEUED (synchronous) vs. active (async) build is unspecified. Document per-state status codes.

---

## Suggestions

- **SSE via `tokio::sync::watch` channel** instead of 500ms file-poll. The log writer publishes `(seq, file_offset)` notifications; the SSE handler `await`s the channel. Lower latency, naturally builds the seq→offset index needed for B8.
- **`cbsd` CLI mode for offline DB operations** (`cbsd api-keys create --name worker-01 --db cbsd.db`). Enables disaster recovery without a running server. Right primitive for migration scripts.
- **`max_token_ttl_seconds` server config** to enforce an upper bound even if the client requests infinite. Lets operators tighten policy without code changes.
- **`component_sha256` in `build_new`** for worker-side integrity verification before unpacking.
- **`descriptor_version` column** (as noted in M6) also enables future migration tooling.
- **Localhost OAuth callback** should be elevated from "future enhancement" to v1 target — ~50 lines in cbc, matches `gh auth login`/`gcloud auth login` UX expectations.

---

## Strengths

- **WebSocket binary frame pairing** (`build_new` JSON + binary tarball + `component_sha256`) is elegant, avoids base64 overhead, and the integrity check catches both transfer corruption and protocol ordering violations.
- **Reconnection decision table** is the right design artifact — covers the hardest cases (dead worker reconnects claiming an active build) and will prevent subtle race conditions during implementation.
- **Argon2/SHA-256 dual hashing** is correct: argon2 for API keys (offline brute-force resistance), SHA-256 for PASETO tokens (per-request, already cryptographically protected). Common mistake avoided.
- **Server-authoritative component model** eliminates the `ENOTRECOVERABLE` startup crash when the Celery worker is unavailable. Removes a hard ordering dependency that caused real operational incidents.
- **Two-phase permission check** (capability at extractor, scope at handler) is idiomatically correct for axum. Clean, auditable authorization code.
- **`PUT` replace-all + `POST` add-one** for user-role assignments is the right API shape for concurrent permission management.
- **Glob over regex for scopes** eliminates the quadruple-escaped YAML maintenance hazard.
- **Admin bootstrapping via `seed_admin`** only on empty DB is safe — no dangerous override on restart.
- **Mutual exclusion invariant** for dispatch is explicitly documented, which prevents premature "optimization" later.
- **Process group SIGTERM propagation** with two-stage escalation to SIGKILL is correct operational behavior for container workloads.

---

## Open Questions

- **What bytes are SHA-256-hashed for `token_hash`?** Raw UTF-8 token string, decoded payload, or binary ciphertext? Must be specified for Python→Rust migration compatibility.
- **What does `GET /api/auth/whoami` return?** Wire format needed — likely the source for `signed_off_by.user` in cbc (per B4).
- **What is the re-dispatch trigger for a build rejected by all workers?** If no worker is idle, nothing triggers re-dispatch until the next `build_finished`. Is there a periodic scan?
- **What is the server's behavior if a sqlx write fails during startup recovery?** Abort? Continue with partial state? Log and skip?
- **What is the server's response if `build_output` arrives after `build_finished`?** Network reordering could flip message order. Buffer, or discard?
- **Does `POST /api/builds` validate component names** against the server's component store at submission time?
- **How is `build_output` ordering guaranteed** when `tokio::fs` async writes may not be atomic? Is fsync required per-append?
- **How is `worker_status` correlated with `worker_id` across restarts** when the server's `workers` map entry has timed out?
- **What is the exact PASETO token payload schema?** Must be frozen as a versioned constant in both Python cbsdcore and Rust cbsd.
