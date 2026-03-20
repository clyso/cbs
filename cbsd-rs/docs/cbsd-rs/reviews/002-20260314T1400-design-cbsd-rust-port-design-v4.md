# Design Review: cbsd Rust Port — v5 (2026-03-15)

**Documents reviewed:**
- `cbsd-rs/docs/cbsd-rs/design/README.md`
- `cbsd-rs/docs/cbsd-rs/design/002-20260313T1800-cbsd-rust-port-design.md`
- `cbsd-rs/docs/cbsd-rs/design/003-20260313T2129-cbsd-auth-permissions-design.md`
- `cbsd-rs/docs/cbsd-rs/design/001-20260313T1800-cbsd-project-structure.md`

---

## Summary

This is a mature design suite that has iterated through multiple review cycles. All prior blockers and major concerns have been addressed. The architecture is sound, the WebSocket protocol is production-quality, the dispatch state machine is correctly specified, and the RBAC model is clean.

Four blockers remain — none architectural. The most critical is B1: the two independent `require_scope` calls in the `create_build` handler create a cross-assignment authorization bypass where different assignments can satisfy different scope types, authorizing combinations no single assignment permits. B2 (`body.descriptor.repo` does not exist on `BuildDescriptor`) will produce a compile error. B3 (the `whoami` example uses `"project"` but the schema defines `"channel"`) is a direct document/schema contradiction. B4 (PASETO payload schema unfrozen) is answerable from the existing Python source and should be closed immediately.

**Verdict: Approve with conditions.** Resolve the 4 blockers and 6 major concerns as targeted fixes. Queue/dispatch, log durability, WebSocket protocol, and startup recovery can proceed immediately.

---

## Blockers

Issues that must be resolved before implementation begins.

### B1 — Multi-scope evaluation creates cross-assignment authorization bypass

The `create_build` handler makes independent `require_scope` calls for channel and registry/repository. The stated semantics are "pass if any assignment matches." Two independent calls can be satisfied by *different* assignments simultaneously.

Example: alice has `builder` with two assignments — A scoped to `channel=ces-devel/*`, B scoped to `registry=harbor.clyso.com/ces-prod/*`. A build targeting `channel=ces-devel/ceph` pushing to `harbor.clyso.com/ces-prod/ceph` passes both checks (A satisfies channel, B satisfies registry), even though no single assignment authorizes that specific combination. This is the classic confused-deputy problem in multi-dimensional RBAC.

**Fix (choose one):**
- **Option A (assignment-level AND — stricter, likely correct):** Replace independent `require_scope` calls with a single `require_scopes` that takes all scope checks and finds one assignment satisfying ALL of them.
- **Option B (per-type OR — looser):** Keep independent checks. Document explicitly that cross-assignment combinations are intentionally permitted. Accept the broader authorization model.

### B2 — `body.descriptor.repo` field does not exist on `BuildDescriptor`

The auth doc's scope check example calls `user.require_scope(ScopeType::Repository, &body.descriptor.repo)`. The Python `BuildDescriptor` has no top-level `repo` field. The `repo` field exists on `BuildComponent` (each descriptor can have multiple components, each with an optional and potentially distinct `repo`). This code will not compile.

**Fix (choose one):**
- **Option A:** Add a top-level `repo: Option<String>` field to the Rust `BuildDescriptor`. Document as a schema change in the migration plan.
- **Option B:** Define an extraction rule (e.g., check each `components[].repo`, pass if any matches).
- **Option C:** Remove `repository` as a scope type from `builds:create` for v1 and rely on channel + registry scopes only.

### B3 — `whoami` response uses `"type": "project"` but the schema defines `"channel"`

The `GET /api/auth/whoami` response example shows `{ "type": "project", "pattern": "ces-devel/*" }`. The `user_role_scopes` table has `CHECK (scope_type IN ('channel', 'registry', 'repository'))` — `"project"` is not a valid value. The scope type was explicitly renamed from `project` to `channel` (documented in Decided Questions). An implementor using this example as a reference will produce a serializer that emits `"type": "project"`, failing the CHECK constraint.

**Fix:** Replace `"type": "project"` with `"type": "channel"` in the `whoami` example. Grep the entire document set for `"project"` as a scope type value and verify each instance.

### B4 — PASETO token payload schema is unfrozen

The `tokens.token_hash` hashing spec is frozen (SHA-256 of raw UTF-8 token string). But the encrypted payload schema — field names, types, `expires_at` encoding, `jti` presence — is still listed as an open question. Both the Python migration script and the Rust server must agree on this.

The answer is already in the codebase. `cbsdcore/src/cbsdcore/auth/token.py` defines:
```python
class TokenInfo(pydantic.BaseModel):
    user: str          # email
    expires: dt | None # ISO 8601 or null
```
No `jti` field exists. The Rust equivalent is:
```rust
struct CbsdTokenPayloadV1 {
    user: String,
    expires: Option<DateTime<Utc>>,
}
```

**Fix:** Add a `PASETO Payload Schema` section to the auth document with the exact field names and types. Mark as a versioned constant (`CBSD_TOKEN_PAYLOAD_V1`). Close the open question.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — API key LRU cache cannot be invalidated on revocation or deactivation

The LRU cache is keyed by SHA-256 of the raw API key string. Two invalidation paths cannot reach the cache:

1. **Individual revocation** (`DELETE /auth/api-keys/{prefix}`): The handler has the `key_prefix`, not the raw key. The argon2 `key_hash` in the DB is one-way — you cannot derive the SHA-256 cache key from it.
2. **Bulk deactivation** (`PUT /admin/users/{email}/deactivate`): The handler has the user's email but the same SHA-256 derivation problem. Even if individual revocation is fixed, bulk deactivation has the same structural gap.

Result: revoked/deactivated API keys remain authenticated via the LRU until natural eviction.

**Fix:** The LRU needs reverse indices:
```rust
struct ApiKeyCache {
    by_sha256: LruCache<[u8;32], CachedApiKey>,
    by_prefix: HashMap<String, [u8;32]>,       // prefix → sha256
    by_owner: HashMap<String, HashSet<[u8;32]>>, // email → set of sha256
}
```
On individual revocation by prefix: use `by_prefix` to find and remove. On bulk deactivation by email: drain `by_owner[email]`. On cache insert: populate both reverse maps. Specify this explicitly in the design.

### M2 — Log file GC policy is completely absent

The Python system GCs Redis log streams after 6 hours. The Rust design writes to disk with no specified size limit, retention policy, or cleanup mechanism. Build logs for Ceph RPM builds are multi-megabyte. A production system running daily builds will fill its log directory in days to weeks. There is no specified behavior for "log directory full."

**Fix:** Add a log retention policy. Minimum: configurable `log_retention_days` (default 7), periodic task (using the existing `tokio-cron-scheduler`) that deletes log files for builds older than the retention window and updates `build_logs` accordingly. Or: explicitly state "operator must configure external log rotation" and close the open question.

### M3 — Stopping worker mid-dispatch: DISPATCHED build not re-queued on connection close

The graceful shutdown section says `Stopping` workers are "deregistered immediately (not placed in the Disconnected grace period)" when their connection closes. But it does not specify what happens to a build in DISPATCHED state assigned to that worker. The build sits in DISPATCHED until... nothing — no timer, no grace period (skipped for Stopping), no reconnect (worker is shutting down).

**Fix:** Add: "When a Stopping worker's connection closes, any build in DISPATCHED state assigned to that worker is immediately re-queued to the front of its priority lane." One sentence.

### M4 — Web UI session-cookie auth is underdefined

The web flow says "The callback sets an `HttpOnly` session cookie and redirects the user to the web UI root." But the `AuthUser` extractor only reads `Authorization: Bearer`. If the web UI sends requests with a session cookie, every endpoint returns 401. If the web UI stores the token in `localStorage` and sends it as Bearer, the session cookie language is misleading.

**Fix:** Pick one and commit. Simplest: the web UI stores the PASETO token in `localStorage`/`sessionStorage` and sends it as `Authorization: Bearer`. Remove or clarify the `HttpOnly session cookie` language to refer only to the OAuth round-trip session, not ongoing API auth.

### M5 — `cli_port` redirection vulnerable to session fixation

The localhost auto-redirect flow stores `cli_port` in the session alongside `oauth_state`. If an attacker can cause a victim to use a pre-established session (by setting the session cookie before the OAuth flow starts), the attacker can set their own `cli_port`, receiving the victim's token.

**Fix:** Add: "At callback, after validating the `state` parameter, regenerate the session ID before issuing the PASETO token." Standard defense; `tower-sessions` supports it.

### M6 — `descriptor_version` has no interpretation spec for unknown versions

The column exists (`INTEGER NOT NULL DEFAULT 1`) but the design doesn't specify: what version 1 structurally means, whether Python-migrated rows receive `DEFAULT 1`, or how the server handles unrecognized versions (reject? best-effort? panic?).

**Fix:** Add a paragraph specifying: (a) version 1 = the `BuildDescriptor` JSON shape as of this design, with `build.arch` accepting both `arm64` and `aarch64`; (b) Python-migrated rows receive `DEFAULT 1`; (c) unrecognized versions return an error indicating a server upgrade may be needed.

---

## Minor Issues

- **`builds.submitted_at` vs `queued_at` redundancy.** For v1 these are always equal. Either collapse to one field or add a comment explaining `queued_at` is reserved for future deferred queuing.
- **`serde_yml = "0.0.12"` is pre-1.0.** Note in the dependency table: "pre-1.0 API, pin to patch version, monitor for 1.0."
- **`http = "1"` in cbsd-worker.** Likely a transitive dep from `tokio-tungstenite`. Verify at `cargo tree` time that the explicit dep adds nothing.
- **`build_logs.log_path` drift risk.** If `log_dir` config changes, stored paths diverge. Document: "log_path must be updated if log_dir changes."
- **sqlx offline query cache must be committed.** Add a workflow note: "Run `cargo sqlx prepare` after any migration or query change. `.sqlx/` is committed. CI fails if stale."
- **`whoami` `effective_caps` doesn't reflect scope context.** Add a note: "scope-gated capabilities still require matching scopes per-request. Use `roles[].scopes` to determine where they apply."
- **Registry scope check absent from `create_build` example.** The scope type table lists `registry` but the handler example omits it. Either add it or explain the omission.
- **OAuth session TTL not specified.** An incomplete flow leaves an orphaned session row indefinitely. Specify a short TTL (e.g., 10 minutes).
- **`tracing-appender` log rotation strategy.** Server process log rotation is unspecified. State whether the server self-rotates or relies on external tools (logrotate, journald).
- **`PUT /permissions/roles/{name}` returns 409 for builtin modification.** Semantically 403 Forbidden may be more appropriate. Minor UX nit.
- **`GET /api/workers` response schema unspecified.** The endpoint is listed but the response shape (fields, types) is never defined.
- **`POST /api/builds/new` stale reference.** The dispatch logic section still references `/builds/new`; the README lists `/builds`. One-line fix.
- **`glob-match` `*` semantics with path separators.** Verify `*` does not match `/`. If it does, `ces-devel/*` matches `ces-devel/subdir/repo`, which may be unintended. Consider `globset` which has well-specified `*` vs `**`.

---

## Suggestions

- **Add `dispatch_queued_at: Instant` to `QueuedBuild` from day one.** The age-based starvation promotion path requires it. Costs nothing now; avoids a DB read per-build later.
- **Verify `tower-governor` uses rolling window, not fixed window.** A fixed-window rate limiter on `/auth/callback` could exhaust the budget for legitimate retries.
- **Add `Content-Security-Policy` to the token-display HTML page.** `default-src 'none'; script-src 'none';` prevents exfiltration via injected scripts.
- **Add a composite index `(finished, updated_at)` on `build_logs`.** Cheap in the initial migration; needed if log GC (M2) queries "unfinished log records older than N hours."
- **Specify HKDF context string for session signing key.** The auth doc says "HKDF-SHA256 with context `cbsd-oauth-session-v1`" — verify that `tower-sessions` accepts a raw byte slice derived this way.

---

## Strengths

- **WebSocket protocol is production-quality.** Reconnection decision table is authoritative, covers all 9 state combinations. "Always revoke unknown" invariant prevents zombie builds.
- **Split-mutex dispatch.** DB write under lock, WS send outside lock, push-to-front on failure. Crash gap is closed. Most designs still hold the lock across I/O.
- **REVOKING state with separate ack timeout (30s) from liveness grace period (90s).** Clean two-phase revoke with unilateral terminal transition on expiry.
- **Auth layering.** SHA-256 for PASETO tokens (per-request), argon2 for API keys (connection setup). LRU cache with SHA-256 keying. `RequireCap` extractor + handler-level scope check is idiomatic axum.
- **Last-admin guard across all five mutation paths.** Exhaustive enumeration with the `?force=true` CASCADE path still threading through the guard.
- **Scope-on-assignment model.** Separates "what you can do" (role caps) from "where you can do it" (assignment scopes). Multi-role union semantics are correct. Validation at assignment time prevents confusing 403s.
- **Component integrity failure → FAILURE, not re-queue.** Server-side tarball corruption fails on every worker identically. Correctly avoids serial rejection cascade.
- **Startup recovery.** DISPATCHED/STARTED → FAILURE on restart (no resurrection attempt). QUEUED builds re-inserted in priority/time order.
- **cbscore subprocess bridge.** Process-group SIGTERM, SIGKILL escalation, structured result line on stdout. Reflects operational experience.
- **Bootstrapping in single transaction.** Roles → admin user → role assignment → API keys. Atomic.
- **Worker reconnect backoff with validated ceiling constraint.** `reconnect_backoff_ceiling < grace_period`, validated at startup.

---

## Open Questions

- **Multi-scope evaluation semantics (also B1).** Assignment-level AND or per-type OR? Must be decided before scope-check implementation.
- **`BuildDescriptor.repo` source (also B2).** Where does the repository scope value come from? First component's repo? Union? New top-level field? Or remove from v1?
- **Web UI ongoing auth mechanism (also M4).** Session cookie or Bearer token in localStorage? Different extractor implementation and security properties.
- **Worker Python environment.** Python version, cbscore install method, `cbscore.config.yaml` injection, upgrade path when cbscore is updated. Near-term deployment blocker.
- **cbscore config on workers.** Vault credentials, signing keys, registry credentials — mounted volume, env vars, or config file? Affects container image spec.
- **`glob-match` `*` semantics.** Does `*` match path separators? Overly permissive matching could grant channel access beyond intent.
- **Build log file naming on ID reset.** If Rust starts from ID 1, collisions with retained Python-era log files. Specify handling.
- **`GET /api/workers` response schema.** Listed but undefined.
- **User account lifecycle / retention.** No deletion path. Post-v1 compliance concern.
