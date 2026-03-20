# Design Review: cbsd Rust Port — v6 (2026-03-15)

**Documents reviewed:**
- `_docs/cbsd-rs/README.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-rust-port-design.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-auth-permissions-design.md`
- `_docs/cbsd-rs/2026-03-14-cbsd-project-structure.md`

---

## Summary

This is a mature design suite. The core architectural decisions are sound, prior blockers have been resolved, and the specification depth (authoritative state-transition tables, frozen wire schemas, split-mutex invariants, migration plan) is well above average. The design is close to implementation-ready.

Four blockers remain — none architectural. The most critical is B1: the PASETO `expires` field serialization differs between Pydantic (`+00:00`) and chrono (`Z`), causing SHA-256 hash divergence that breaks the frozen `CBSD_TOKEN_PAYLOAD_V1` contract. B2 (server graceful shutdown sends `build_revoke`, contradicting the rolling-restart guarantee) will terminate all in-flight builds on every deploy. B3 (`ApiKeyCache` multi-map has no mutex) is a concurrency correctness issue. B4 (`BuildDescriptor.arch` nesting is unspecified in the Rust struct) risks silent dispatch failures.

**Verdict: Approve with conditions.** Resolve the 4 blockers and 4 major concerns as targeted fixes. Queue/dispatch, log streaming, WebSocket protocol, and startup recovery can proceed immediately.

---

## Blockers

Issues that must be resolved before implementation begins.

### B1 — PASETO `expires` serialization format is not actually pinned

The auth doc marks `CBSD_TOKEN_PAYLOAD_V1` as frozen, but the exact byte sequence diverges between runtimes. Python's `pydantic_core.to_jsonable_python` serializes datetimes as `"2026-03-14T10:30:00+00:00"`. Rust's `chrono::DateTime<Utc>` with default serde produces `"2026-03-14T10:30:00Z"`. The `+00:00` vs `Z` suffix means the JSON payload bytes differ, SHA-256 hashes differ, and the frozen contract is violated.

Additionally, JSON key ordering is not mandated by the spec. If Pydantic produces `{"user": ..., "expires": ...}` and Rust serde derive produces `{"expires": ..., "user": ...}`, the hash also diverges.

Even for the hard-cutover path (users re-auth), the Rust issuer and validator must agree with each other. Any future audit reproduction of a token hash will fail without a pinned byte sequence.

**Fix:** Pin the exact serialization. Simplest: serialize `expires` as a Unix epoch integer (`i64`) — no timezone ambiguity, no fractional seconds, no `Z` vs `+00:00`. If ISO 8601 is required, mandate `Z`-suffix, no sub-second precision, and add a cross-language test comparing SHA-256 output for identical payloads. Also pin JSON key ordering (alphabetical, or define a canonical form).

### B2 — Server graceful shutdown sends `build_revoke` — contradicts rolling restart guarantee

The shutdown sequence says: "Send `build_revoke` to all workers with active builds" and then "Workers will detect the connection drop and enter their reconnection loop, re-attaching to the new server instance." These directly contradict: if the server sends `build_revoke`, the worker kills the running container, sends `build_finished(revoked)`, and the build is terminally `REVOKED`. The new server instance finds a terminal build and does not re-queue it. Every rolling deploy terminates all in-flight builds.

The reconnection decision table handles `STARTED + worker reconnects building N` correctly (resume, no state change) — but only if the previous server did NOT send `build_revoke`.

**Fix:** Split into two modes:
1. **Graceful restart** (default SIGTERM): Do not send `build_revoke`. Flush logs to disk. Close WebSocket connections. Workers enter reconnection loop and re-attach to the new instance. The reconnection table handles reconciliation.
2. **Intentional decommission** (`--drain` flag or explicit signal): Send `build_revoke`, wait for acks, mark unacknowledged builds `FAILURE("server decommissioned")`.

### B3 — `ApiKeyCache` multi-map operations have no mutex

The `ApiKeyCache` struct holds three maps (`by_sha256`, `by_prefix`, `by_owner`) that must be updated atomically. It is shared across request handlers via `Arc<AppState>`. No mutex is specified. Without one: concurrent LRU eviction from `by_sha256` leaves stale entries in `by_prefix`; revocation via `by_prefix` races with insert; bulk deactivation draining `by_owner` races with new requests caching the same key.

**Fix:** Specify `Arc<Mutex<ApiKeyCache>>` (Tokio async mutex). The mutex must be held across all multi-map operations atomically. Document that LRU eviction from `by_sha256` must also clean up `by_prefix` and `by_owner` (via an `on_evict` callback or overridden pop path).

### B4 — `BuildDescriptor.arch` is nested at `descriptor.build.arch` — Rust struct layout unspecified

The dispatch logic says "find a worker whose `arch` matches the build target." In the Python model, arch lives at `BuildDescriptor.build.arch` (inside nested `BuildTarget`), not at the top level. The `cbsd-proto` crate description mentions `BuildDescriptor` but shows no struct layout. If the Rust struct flattens this, it fails to deserialize Python-era `builds.descriptor` JSON blobs (which have `{"build": {"arch": "arm64", ...}}`). If it preserves nesting, the dispatch comparison must read `descriptor.build.arch`.

**Fix:** Add the full Rust `BuildDescriptor` struct layout to the design doc. Map every Python field explicitly, including nested `BuildTarget`. Confirm the dispatch pseudocode reads `descriptor.build.arch`. Note that the `arm64`/`aarch64` serde alias covers deserialization, but only if the struct layout is correct.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — No TLS configuration path for non-public-CA certificates

`cbsd-worker` uses `tokio-tungstenite` with `rustls-tls-native-roots`, validating against the OS trust store. CBS is an internal build tool — internal infrastructure commonly uses self-signed certs or private CAs not in the OS trust store. There is no config option for a custom CA bundle. The first worker container in a dev/lab deployment will refuse to connect with a TLS error.

**Fix:** Add `tls_ca_bundle_path: Option<PathBuf>` to worker config. When set, load the PEM and add it as a trusted root to the rustls `ClientConfig`. ~10 lines of code.

### M2 — Log GC has a TOCTOU window that truncates active SSE streams

The GC task deletes log files for terminal builds older than `log_retention_days`. If a client connects to `GET /builds/{id}/logs/follow` for a completed build, and GC deletes the file between the SSE handler's "check if file exists" and "open file" steps, the client receives a 404 or server error with no `event: done` marker. The stream just stops.

**Fix:** Two complementary changes: (1) In the SSE handler, if the log file does not exist for a terminal build, emit a synthetic `event: done` immediately (log was GC'd, build is complete). (2) Use a `gc_at` timestamp or `finished=1` check as a tighter GC guard.

### M3 — `build_revoke` before `build_accepted`: worker behavior undefined

If `DELETE /builds/{id}` is called while the build is in `DISPATCHED` state, the server sends `build_revoke`. But the worker may still be processing `build_new` and unpacking the component tarball. The worker receives `build_revoke` before it has sent `build_accepted`. The protocol does not specify what the worker should do in this case. Without a rule, the worker may ignore the revoke (no "active" build yet), resulting in `build_accepted` arriving after the server has already transitioned to `REVOKING`.

**Fix:** Add a protocol rule: "If `build_revoke` arrives before `build_accepted` has been sent, the worker immediately responds with `build_finished(revoked)` without starting execution."

### M4 — `trace_id` is not propagated to the cbscore subprocess

The design says `trace_id` from `build_new` is "included in all worker-side log output via tracing spans." But cbscore is a separate Python process. The `trace_id` is not included in the stdin JSON payload specification. All cbscore log output — the most verbose part of the build — will lack the `trace_id`, making cross-boundary log correlation impossible for the phase that matters most.

**Fix:** Include `trace_id` in the stdin JSON passed to the Python wrapper. The wrapper sets it as an environment variable (e.g., `CBS_TRACE_ID`) for cbscore's logging layer.

---

## Minor Issues

- **`serde_yml = "0.0.12"` is pre-1.0.** Pin the exact version in workspace `Cargo.toml`. Test upgrades deliberately.
- **`submitted_at` vs `queued_at` redundancy.** Both always equal in v1. Make the migration SQL comment explicit that v1 writes the same value to both.
- **`build_logs.log_path` drift scenario.** If `log_dir` config changes, stored paths diverge. Add an operations note: changing `log_dir` requires a SQL update + file move.
- **`DELETE /permissions/roles/{name}?force=true` + last-admin violation.** Clarify: returns 409 Conflict (same as without `?force`), not 500.
- **`GET /auth/api-keys` has no admin override.** An admin auditing a user's keys has no endpoint. Add `?owner=<email>` gated on `permissions:manage`, or note the absence explicitly.
- **Token payload JSON key ordering not specified.** Reinforces B1 — pin the serialization form in a cross-language test.
- **`Stopping` state missing from the worker state machine diagram.** Add it with the "skip grace period on disconnect" edge.
- **`PUT /permissions/users/{email}/roles` replace-all is a footgun.** Consider `?dry-run=true` parameter to reduce operational risk.
- **`DELETE /auth/api-keys/{prefix}` ambiguous when two users share a prefix.** Admin path needs `?owner=<email>` or docs must clarify scoping.
- **`GET /builds/{id}` response shape unspecified.** Field names change from Python (`task_id` → dropped, `submitted` → `submitted_at`, `desc` → `descriptor`, `user` → `user_email`, states lowercase). Add a JSON example and add field changes to the "breaking changes for cbc" list.
- **`build_logs.log_size` periodic update (5s) not assigned to a module.** Assign to `writer.rs` or a central timer. Stagger by `build_id` modulo to avoid write bursts.
- **`tower-sessions-sqlx-store` uses its own connection(s).** Note that WAL mode is per-database (inherited), but `foreign_keys=ON` is per-connection (not inherited by the sessions library — irrelevant since sessions have no FKs).
- **`nix::unistd::setsid()` must be called in `Command::pre_exec()`.** Note this in the design — it's an unsafe block and a common first-time Rust subprocess mistake.
- **`sqlx::migrate!("../migrations")` relative path.** Document the workspace layout constraint (path is relative to `CARGO_MANIFEST_DIR`).
- **`watch::Receiver` when sender is dropped.** Specify SSE handler behavior on `RecvError`: check `build_logs.finished` from DB, emit `event: done` if terminal, `event: error` if non-terminal.
- **Session TTL not specified.** Incomplete OAuth flows leave orphaned session rows. Specify a short TTL (e.g., 10 minutes).
- **Web UI ongoing auth mechanism.** The web flow mentions `HttpOnly session cookie` for ongoing API auth, but `AuthUser` only reads `Authorization: Bearer`. Either extend the extractor or clarify that the web UI stores the token in localStorage and sends it as Bearer (simpler, likely intended).

---

## Suggestions

- **`cbsd-server admin bootstrap` subcommand** for non-interactive provisioning. Useful for container entrypoints and Ansible. First-startup seeding via normal server run is awkward in automation.
- **Partial index `(finished_at)` for terminal builds** on the GC query. Not needed at current scale but cheap in the initial migration.
- **`cbsd-proto` zero-dependency constraint** should be enforced in CI (e.g., `cargo deny` or a workspace-level test).
- **Consider separating the cbscore result JSON onto fd 3** instead of mixing with stdout. Eliminates per-line JSON prefix inspection.
- **Explicit `Content-Security-Policy`** on the token-display HTML page: `default-src 'none'; script-src 'none'`.
- **`dispatch_queued_at: Instant` in `QueuedBuild`** from day one. Costs nothing; makes age-based starvation promotion trivial to add later.

---

## Strengths

- **Reconnection decision table + grace period expiry table.** Authoritative, complete, cross-referenced in the README. Every cell has a specified action with rationale for non-obvious entries.
- **Split-mutex dispatch with DB write under lock.** Closes the crash gap. Fully specified with failure recovery (re-acquire, push to front).
- **`CBSD_TOKEN_PAYLOAD_V1` frozen constant.** Right instinct — cross-language type alignment explicitly documented. The remaining serialization pinning is a refinement, not a flaw.
- **Assignment-level AND for scope evaluation.** Non-obvious, easy to implement wrong. The confused-deputy example and `require_scopes_all` pseudocode leave no ambiguity.
- **REVOKING state with 30s ack timeout.** Clean separation from 90s liveness grace period. Late `build_output` after `finished=1` discarded.
- **`ApiKeyCache` three-map reverse-index design.** Structurally correct for prefix-based revocation and email-based bulk deactivation (only the mutex wrapping is missing).
- **Last-admin guard across all five mutation paths.** Exhaustive enumeration. `?force=true` CASCADE still threads through the guard.
- **cbscore subprocess bridge.** Process-group SIGTERM, SIGKILL escalation, structured result line. Reflects operational experience.
- **Component integrity failure → FAILURE, not re-queue.** Prevents serial rejection cascade across all workers.
- **`cbsd-proto` as zero-dependency shared-types crate.** Compile-time protocol agreement at zero runtime cost.
- **Session fixation prevention.** Session regeneration at callback specified — non-obvious security requirement that many OAuth implementations miss.
- **Worker reconnect backoff with validated ceiling constraint.** `ceiling < grace_period`, validated at startup.

---

## Open Questions

- **PASETO `expires`: epoch seconds or ISO 8601?** Resolves B1. Must be decided before auth implementation.
- **Rust `BuildDescriptor` struct layout.** Does `arch` live at `descriptor.build.arch` (matching Python) or is it promoted? Resolves B4.
- **Server shutdown: restart vs decommission modes?** Resolves B2. Should `--drain` flag select decommission? Default SIGTERM = restart?
- **`glob-match` `*` semantics.** Version 0.2 matches `*` to path separators. `ces-devel/*` would match `ces-devel/foo/bar`. Consider `globset` for explicit `*` vs `**` handling.
- **Worker container image spec.** Python version, cbscore install method, config injection, Vault access. Near-term deployment blocker.
- **Duplicate `worker_id` display labels.** Should the server warn on connection if two active workers use the same label?
- **Seq-to-offset index on server restart.** For builds transitioning to FAILURE during recovery, the in-memory index is gone. SSE follow falls back to linear scan. Document as known limitation.
- **`GET /api/workers` response schema.** Listed but undefined. Should include: connection UUID, worker_id label, arch, state, current build_id, connection duration.
- **`log_size` purpose.** Purely informational, or used in admission control? Determines whether the 5s update frequency matters.
