# Design Review: cbsd Rust Port — Final Pre-Implementation Review

**Documents reviewed:**
- `_docs/cbsd-rs/README.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-rust-port-design.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-auth-permissions-design.md`
- `_docs/cbsd-rs/2026-03-14-cbsd-project-structure.md`

---

## Summary

This design suite has reached a high degree of maturity after seven review rounds. The protocol is well-specified, the failure modes are enumerated, the RBAC model is clean, and the state machine tables are authoritative. Roughly 90% of the surface area is implementation-ready.

Three blockers remain. The most critical is B1: direct inspection of the Python source confirms that `pydantic_core.to_jsonable_python` serializes `TokenInfo.expires` as ISO 8601 (`"2024-03-14T12:30:00+00:00"`), not as the epoch integer the frozen `CBSD_TOKEN_PAYLOAD_V1` spec mandates — this is a provable code-level divergence, not a theoretical concern. B2 (Rust `BuildDescriptor` drops `version_type` and `artifact_type` fields present in Python) causes silent data loss on round-trip. B3 (`ApiKeyCache` LRU eviction cleanup is unimplementable as specified because `CachedApiKey` lacks `key_prefix` and the `lru` crate has no `on_evict` callback).

**Verdict: Approve with conditions.** Resolve B1–B3 and address the major concerns during initial implementation. The architecture does not need to change.

---

## Blockers

Issues that must be resolved before implementation begins.

### B1 — PASETO `expires` serialization: frozen spec says epoch integer, running Python produces ISO 8601

The frozen `CBSD_TOKEN_PAYLOAD_V1` spec mandates:
```json
{"expires":1710412200,"user":"alice@clyso.com"}
```

The actual Python code path (`cbsd/cbslib/auth/auth.py`) calls `pydantic_core.to_jsonable_python(TokenInfo)` and passes the result to `pyseto.encode`. Pydantic v2's default datetime serialization produces ISO 8601 (`"2024-03-14T12:30:00+00:00"`), not an integer. The `pyseto.encode` call receives `{"user": "alice@clyso.com", "expires": "2024-03-14T12:30:00+00:00"}`. The SHA-256 of every token currently in the field is computed over these ISO 8601 payload bytes.

The design's Python pseudocode (`json.dumps({"expires": int(dt.timestamp()) ...}, sort_keys=True, separators=(",", ":"))`) is aspirational — the actual implementation does not call `json.dumps` at all. The CI cross-language hash test, as described, will verify freshly-constructed payloads match each other but will not catch the divergence with existing tokens.

Additionally, JSON key ordering is not intrinsically guaranteed. `serde_json` serializes fields in struct declaration order (which happens to be alphabetical for `{"expires", "user"}`), but this is coincidental. A future payload field between "e" and "u" will only appear in alphabetical position if the Rust struct declares it in that order.

**Fix:** Before writing `auth/paseto.rs`:
1. Decrypt a live Python-issued token and confirm the actual payload bytes.
2. Update the auth doc to note: "The current Python server produces ISO 8601 `expires`. `CBSD_TOKEN_PAYLOAD_V1` specifies epoch integers. These are not hash-compatible. The migration break invalidates all existing tokens regardless."
3. Remove or demote the zero-downtime migration path (it requires hashing ISO 8601 payloads, which the spec does not support).
4. The CI cross-language hash test must assert exact byte sequences, not rely on emergent field ordering.

### B2 — Rust `BuildDescriptor` drops `version_type` and `artifact_type` — silent data loss on round-trip

The Python `BuildDescriptor` (`cbsdcore/src/cbsdcore/versions.py`) has two fields absent from the Rust struct in the design:

1. `version_type: VersionType` — required field, values `"release"`, `"dev"`, `"test"`, `"ci"`. Present in every existing `cbc` submission.
2. `artifact_type: BuildArtifactType` inside `BuildTarget` — string enum, default `"rpm"`.

With default `#[derive(Deserialize)]` (no `deny_unknown_fields`), these fields are silently dropped. If migrated `builds.descriptor` blobs are ever re-serialized (display, re-dispatch, periodic re-submission), the fields are permanently lost. `version_type` has semantic meaning — cbscore may use it for tagging or behavior selection.

**Fix:** Add both fields to the Rust structs:
```rust
struct BuildDescriptor {
    version: String,
    channel: String,
    version_type: String,  // "release" | "dev" | "test" | "ci"
    signed_off_by: BuildSignedOffBy,
    dst_image: BuildDestImage,
    components: Vec<BuildComponent>,
    build: BuildTarget,
}
struct BuildTarget {
    distro: String,
    os_version: String,
    artifact_type: String,  // default "rpm"
    arch: Arch,
}
```
If the Rust server doesn't use them, mark `#[allow(dead_code)]` with a comment. The cbscore subprocess wrapper must pass them through.

### B3 — `ApiKeyCache` LRU eviction cleanup is unimplementable as specified

The design says eviction should use "a custom `pop` wrapper or `on_evict` callback." The `lru` crate (0.12) has no `on_evict` callback. Eviction happens implicitly via `push()`, which returns the evicted key-value pair. The `CachedApiKey` struct is defined as `{owner_email, roles, expires_at}` — missing `key_prefix`. Without `key_prefix` in the evicted value, the cleanup cannot find and remove the corresponding entry from `by_prefix`.

Result: after LRU eviction, `by_prefix` retains a stale SHA-256 → entry mapping. A subsequent `DELETE /auth/api-keys/{prefix}` finds the prefix, retrieves the SHA-256, looks it up in `by_sha256` (cache miss — evicted), and silently fails to invalidate. `by_owner` retains the stale SHA-256 too, corrupting bulk deactivation.

**Fix:** Add `key_prefix: String` to `CachedApiKey`. Specify the exact eviction pattern:
```rust
fn insert(&mut self, sha256: [u8; 32], entry: CachedApiKey) {
    if let Some((evicted_sha256, evicted)) = self.by_sha256.push(sha256, entry.clone()) {
        self.by_prefix.remove(&evicted.key_prefix);
        if let Some(set) = self.by_owner.get_mut(&evicted.owner_email) {
            set.remove(&evicted_sha256);
            if set.is_empty() { self.by_owner.remove(&evicted.owner_email); }
        }
    }
    self.by_prefix.insert(entry.key_prefix.clone(), sha256);
    self.by_owner.entry(entry.owner_email.clone()).or_default().insert(sha256);
}
```
This is the only correct pattern for `lru 0.12`. Replace the vague language in the design with this concrete implementation.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — Token migration: zero-downtime path is underspecified and misleading

The auth doc offers "accept the break" (recommended) and a zero-downtime alternative ("import existing token JTIs from the Python dbm store"). The alternative is unimplementable as written: the `dbm` key schema is undocumented, the hash computation procedure is absent, and the ISO 8601 / epoch integer divergence (B1) means imported hashes would be wrong anyway.

**Fix:** Demote the zero-downtime path to a footnote: "Not supported for v1; users must re-authenticate after cutover." Remove the implication that it is a turnkey option.

### M2 — GC + SSE race: mitigation insufficient for partial-read truncation

The "synthetic `event: done` on missing file" mitigation handles the case where the file doesn't exist at open time. It doesn't handle the case where the SSE handler has the file open, yields to tokio, and GC deletes the file between reads.

On Linux, an open FD to an unlinked file continues to work (the inode lives until all FDs close). The fix is a design constraint, not a code change: the SSE handler must open the file once by FD at stream start and hold it open for the lifetime of the stream. This eliminates the race entirely. Document this as a design constraint on `logs/sse.rs` before implementation.

### M3 — `tokio::sync::Mutex<BuildQueue>` held across SQLite write — pool starvation risk

The dispatch mutex is held across an `sqlx` write (1–5ms on NVMe-backed WAL SQLite). If the SQLite pool is at capacity, the `sqlx::query!` await stalls indefinitely while holding the mutex. Every other queue operation waits for the mutex. If any waiting operation also needs a pool connection, deadlock occurs.

**Fix:** Set `min_connections = 1, max_connections = 4` (or similar) on the SQLite pool at startup and document this as a correctness requirement. Also correct the "sub-microsecond" comment — the critical section is bounded by I/O (1–5ms), which is why `tokio::sync::Mutex` (async) is required instead of `std::sync::Mutex`.

### M4 — `GET /builds/{id}` response shape unspecified — `cbc` breakage undocumented

The response fields change from Python: `task_id` dropped, `submitted` → `submitted_at`, `desc` → `descriptor`, `user` → `user_email`, states lowercase. None listed in "Additional breaking changes for cbc." `cbc build list` and `cbc build status` will fail to deserialize.

**Fix:** Add a `GET /builds/{id}` response JSON example to the design. Add field name/type changes to the breaking changes list.

### M5 — Error response body schema undefined — `cbc` expects `{"detail": "..."}`

The current Python server uses FastAPI's `{"detail": "..."}` shape. The Rust server's error format is unspecified. If it uses a different shape, `cbc` will fail to surface error messages.

**Fix:** Define the error response schema explicitly: `{"detail": "human-readable message"}`. If the shape changes, add to breaking changes.

### M6 — Activate/deactivate idempotency with last-admin guard is undefined

`PUT /admin/users/{email}/deactivate` on an already-deactivated admin user: the guard query counts remaining active `*` holders, but this user is already `active=0` and not counted. If they were the only admin (already deactivated), the guard may incorrectly trigger 409 on a no-op.

**Fix:** Document: activate/deactivate are idempotent. If `active` is already in the target state, return 200 immediately without running the guard or bulk revocation.

---

## Minor Issues

- **`serde_yml = "0.0.12"` is pre-1.0.** Pin exact version in workspace `Cargo.toml`.
- **`cbsd-proto` should disable `chrono` default features.** Use `chrono = { version = "0.4", default-features = false, features = ["serde"] }` to avoid pulling in `clock`/`time` deps.
- **`tokio-cron-scheduler` vs `tokio::time::interval` for log GC undecided.** `tokio::time::interval` is sufficient for v1 (only periodic task, zero extra deps). Decide before writing `logs/gc.rs`.
- **No index on `builds.descriptor_version`.** Future bulk-transform migrations will full-scan without it. Add to initial schema.
- **`DELETE /auth/api-keys/{prefix}` scoping rule absent from README.** Admin deletion must scope to owner email. Add to API table.
- **`builds.queued_at` is nullable but used as `ORDER BY` key for startup recovery.** SQLite sorts NULL first. Add `NOT NULL DEFAULT (unixepoch())` or document migration script obligation.
- **`?force=true` + last-admin violation response not specified.** Clarify: returns 409 (same as without `?force`).
- **`GET /auth/api-keys` has no admin audit endpoint.** No way for an admin to list another user's keys.
- **`whoami` `effective_caps` doesn't indicate scope-gated capabilities.** Add a note that scope-gated caps require matching scopes per-request.
- **Missing `scopes` key vs empty `scopes` array not equated.** Specify: both are semantically identical. Use `#[serde(default)]` on the `scopes` field so absent = `Vec::new()`.
- **SSE `Content-Type` header not specified.** Must be `text/event-stream; charset=utf-8`.
- **Session TTL not specified.** Incomplete OAuth flows leave orphaned rows. Specify ~10 minute TTL.
- **`Stopping` state absent from worker state machine diagram.** Add it.
- **`hello` message omits future-proofing fields.** Adding `cores_available`, `ram_available_mb`, or `build_slots_used` now (unused in v1) avoids a protocol version bump for load-aware dispatch later.
- **`welcome` message carries no `connection_id`.** Adding the server-assigned UUID enables `GET /workers` correlation with worker-side tracing.
- **`PUT /permissions/users/{email}/roles` replace-all atomicity.** The delete + insert sequence must be in a single transaction that rolls back completely on any failure.
- **`?force=true` CASCADE + guard must be in one serializable transaction.** Concurrent role assignment between CASCADE and guard check could produce false-negative.
- **Component store has no startup integrity check.** A partially written component directory is loaded silently. A startup scan that packs each component and logs warnings would surface corruption early.
- **`build_logs.log_path` drift detection.** Add a startup check that validates a sample of stored paths against current `log_dir`.
- **Custom role `*` removal via `PUT /permissions/roles/{name}` should be explicitly listed in last-admin guard mutation table.** Currently implicit.

---

## Suggestions

- **Composite index `(state, queued_at)` for startup recovery.** `O(log n + k)` vs `O(n)` as build history grows. Trivial to add in initial schema.
- **`UNIQUE(name, owner_email)` on `api_keys`.** Prevents confusing duplicate-named keys per user.
- **Resolve `components/` mount vs bake-in.** Server-as-source-of-truth implies volume mount. Baking in creates image-rebuild coupling.
- **`cbsd-server admin bootstrap` subcommand** for non-interactive provisioning in container entrypoints and Ansible.
- **`tower-governor` rate limit: verify rolling window, not fixed window.** Fixed windows cause budget exhaustion on boundary.
- **`Content-Security-Policy` on token-display HTML page.** `default-src 'none'; script-src 'none'`.
- **`sqlx::migrate!("../migrations")` relative path constraint.** Document in project structure — path is relative to `CARGO_MANIFEST_DIR`.

---

## Strengths

- **Protocol completeness.** The WebSocket message set is fully specified. The reconnection decision table and grace period expiry table are authoritative. Edge cases (build_revoke before build_accepted, late build_output, server crash between dispatch and DB write, queued + worker claims build) are all named and handled.
- **Split-mutex dispatch.** Pop under lock, SQLite write under lock, release, WS send outside lock, push-to-front on failure. Crash gap closed. This is the correct pattern.
- **Security choices.** SHA-256 vs argon2 with explicit rationale. LRU cache with reverse indices. Session fixation prevention via `cycle_id()`. Bearer-in-header for worker WS upgrade (never query string).
- **RBAC model.** Per-assignment scopes. Assignment-level AND semantics. Confused-deputy example. Last-admin guard across all 5 mutation paths including the subtle custom-role `*` removal path.
- **Two shutdown modes.** SIGTERM = graceful restart (no revocation, workers reconnect). `--drain` = intentional decommission (revoke, mark FAILURE). Prevents build cancellation on rolling deploys.
- **Startup recovery.** DISPATCHED/STARTED → FAILURE. QUEUED re-inserted in priority/time order. `foreign_keys=ON` per-connection note.
- **cbscore subprocess bridge.** Process-group SIGTERM via `setsid`, SIGKILL escalation, structured result line, classified exit codes.
- **Component distribution.** Server-as-single-source-of-truth. ~6 KB tarballs. SHA-256 integrity check with reject-on-mismatch → FAILURE (not re-queue).
- **Log streaming.** Per-line seq, seq→offset index, watch-channel wakeup, binary-search seek. Production-quality.
- **`cbsd-proto` crate discipline.** No IO, no async runtime. Compile-time protocol agreement.
- **Session signing key derivation.** HKDF-SHA256 with stable context string.
- **Schema quality.** Integer epochs. CHECK constraints. `descriptor_version`. `user_role_scopes` with FK cascade.

---

## Open Questions Requiring Pre-Implementation Answers

1. **What does the running Python server actually produce for PASETO `expires`?** Decrypt a live token and log `bytes(decoded_token.payload)`. Resolves B1.
2. **Is the zero-downtime token migration path supported for v1?** If no, remove it from the auth doc. Resolves M1.
3. **Is `version_type` passed to cbscore via the subprocess stdin JSON?** If cbscore uses it for tagging/behavior, it must be in the subprocess payload. Resolves B2 implementation detail.
4. **Worker Python environment.** Python version, package manager, `cbscore.config.yaml` injection, Vault access. Near-term deployment blocker.
5. **`glob-match` `*` semantics.** Version 0.2 matches path separators. `ces-devel/*` → `ces-devel/foo/bar`. Verify before scope-check code ships.
6. **`GET /builds/{id}/logs` (full download) behavior for in-progress builds.** Stream partial content or wait for completion?
7. **Does `CachedApiKey` include `key_prefix`?** Must confirm before writing `auth/api_keys.rs`. Resolves B3.
8. **`GET /api/workers` response schema.** Listed but undefined.
