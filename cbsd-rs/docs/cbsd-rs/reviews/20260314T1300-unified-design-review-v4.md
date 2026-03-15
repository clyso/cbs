# Design Review: cbsd Rust Port — v4 (2026-03-14)

**Documents reviewed:**
- `_docs/cbsd-rs/README.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-rust-port-design.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-auth-permissions-design.md`
- `_docs/cbsd-rs/2026-03-14-cbsd-project-structure.md`

---

## Summary

This is the fourth iteration and the design has matured substantially. All prior blockers and major concerns have been closed. The architecture is sound, decisions are explicitly logged, and the edge cases that matter most (reconnection table, revoke ack, dispatch-mutex split, seq-to-offset index) are specified with enough precision to implement against.

Four blockers remain — none requiring architectural rethink. The most critical is B1: `PRAGMA foreign_keys = ON` is absent from the SQLite connection setup, silently disabling all FK constraints and `ON DELETE CASCADE` rules that the schema relies on in at least seven places. B2 (the `arm64`/`aarch64` enum mismatch with the existing Python codebase) will break all ARM builds at cutover. B3 and B4 address the last-admin guard and endpoint path inconsistencies. All have bounded, clear fixes.

**Verdict: Approve with conditions.** Resolve the 4 blockers and the 5 major concerns as targeted fixes — no full review pass needed.

---

## Blockers

Issues that must be resolved before implementation begins.

### B1 — `PRAGMA foreign_keys = ON` is absent; all FK constraints are silently disabled

SQLite does not enforce foreign key constraints or `ON DELETE CASCADE` unless `PRAGMA foreign_keys = ON` is set on each connection. The startup pragma section lists `journal_mode=WAL` and `busy_timeout=5000` but not `foreign_keys`. sqlx does not enable this automatically.

The schema relies on `ON DELETE CASCADE` in at least seven places (role_caps, user_roles, user_role_scopes, build_logs, etc.). Without the pragma: `DELETE FROM roles WHERE name = 'builder'` silently leaves all `role_caps`, `user_roles`, and `user_role_scopes` rows in place as orphans. The last-admin guard for role deletion — which explicitly relies on detecting cascade effects — returns wrong results. All `REFERENCES` constraints that prevent orphan inserts also silently do nothing.

**Fix:** Add `PRAGMA foreign_keys = ON` as the third pragma in connection setup. Note that this must be set per-connection — use `SqliteConnectOptions::pragma("foreign_keys", "ON")` in the sqlx pool configuration.

### B2 — `BuildArch` enum mismatch: Python uses `"arm64"`, Rust design specifies `"aarch64"`

The existing `cbsdcore/src/cbsdcore/versions.py` defines `BuildArch.arm64 = "arm64"`. The Rust design specifies `"aarch64"` throughout (arch doc, project structure doc, hello message example). This is the wire value serialized into `BuildDescriptor.build.arch`.

Impact: (1) Existing `cbc` clients serialize `"arm64"` — the Rust server either fails to deserialize (422) or accepts it but never matches workers advertising `"aarch64"`. All ARM builds are broken at cutover. (2) Historical build records in the `builds` table contain `"arm64"` in their JSON blobs — these fail to deserialize. (3) The migration plan doesn't mention this change.

**Fix:** Either (a) add `#[serde(alias = "arm64")]` to the Rust `Aarch64` variant so both values are accepted on input, with `"aarch64"` as the canonical serialized form; or (b) update `cbsdcore`'s `BuildArch.arm64` to `"aarch64"` as part of the coordinated release and document it as a breaking change. Option (a) is lower-risk. Add to the migration plan's "Additional breaking changes" section either way.

### B3 — Last-admin guard on deactivation prevents deactivating *any* admin, not just the last

The guard table entry for `PUT /admin/users/{email}/deactivate` reads: "Check if user has `*`". This unconditionally refuses deactivation of any user holding `*` — regardless of whether other active admins exist. An admin whose Google account is compromised cannot be deactivated through the API if they hold `*`. The bulk token + API key revocation that deactivation triggers is unavailable.

This was flagged in a prior review but the guard logic was not corrected — it should be "after applying this deactivation, does at least one active user still hold `*`?", not "does this user have `*`?".

**Fix:** Change the guard to: implement as a transaction that sets `active = 0`, queries `SELECT COUNT(*) FROM users u JOIN user_roles ur ... JOIN role_caps rc ... WHERE u.active = 1 AND rc.cap = '*'`, and rolls back with 409 if the count is zero. Update the guard table entry to match.

### B4 — Deactivation endpoint path inconsistent across documents

The README API table and auth doc REST section both place deactivation at `PUT /api/admin/users/{email}/deactivate`. However, the auth doc's "User deactivation" prose section describes the operation without specifying the path prefix, making it ambiguous whether this belongs under `/admin/` or `/permissions/users/`. A developer working from the auth doc alone could implement the wrong path.

**Fix:** In the auth doc's "User deactivation" section, add an explicit forward reference: "This is exposed as `PUT /api/admin/users/{email}/deactivate`." One sentence eliminates the ambiguity.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — Scope enforcement gap: multi-role users with mixed scope coverage

The auth design says scope checks use "the scopes attached to the specific assignment that granted the capability." For a user with the `builder` role assigned twice (scope A: `ces-devel/*`, scope B: `ces-prod/*`), a build targeting `ces-prod` will incorrectly return 403 if the check hits assignment A first and stops. The correct behavior — scan all assignments granting the capability, pass if any scope matches — is not specified.

**Fix:** Specify explicitly: a scope check passes if **any** assignment that grants the required capability has a scope pattern matching the requested value. Adjust the `require_scope` code example to reflect that it receives the user's full set of assignments.

### M2 — Scope check example references `body.project`, a field that doesn't exist in `BuildDescriptor`

The auth doc's scope check example calls `user.require_scope(ScopeType::Project, &body.project)`. The Python `BuildDescriptor` has no `project` field — it uses `channel`. If the implementer interprets this as "rename `channel` to `project`", they introduce an undocumented breaking change in `BuildDescriptor` serialization.

The second check (`body.descriptor.dst_image.name`) also needs clarification: is the registry scope checked against the full image name, the registry hostname, or the component repo URL? The Python system checks project (channel) and repository (component repo URL), not destination image registry.

**Fix:** Correct the example to use `&body.channel` (or document the rename in the migration plan). Add a note specifying what string is passed for each scope type.

### M3 — Reconnection decision table missing rows for `queued` state and grace-period expiry

The table is declared authoritative but has two gaps:

1. **`queued` + worker claims building N.** If the server crashes between in-memory dispatch and SQLite write, the DB record remains `queued`. On restart + worker reconnect, no table row matches. Fix: add row — `queued` + worker building N → send `build_revoke` immediately.

2. **Grace period expiry (no reconnect).** The table only covers reconnection cases. The transitions `dispatched → failure("worker lost")`, `started → failure("worker lost")`, `revoking → revoked (unilateral)` appear only in prose, not the table. Fix: add a supplementary "grace period expiry" section or table to the authoritative specification.

Also: the dispatch logic doesn't specify when the SQLite write from `queued` to `dispatched` happens relative to the mutex. Fix: specify that the DB write happens under the mutex lock (step 4), before release (step 5).

### M4 — Worker reconnect backoff ceiling must be less than liveness grace period

The project structure doc lists `connection.rs` as implementing a "reconnect loop, backoff." No backoff parameters appear anywhere. If the backoff ceiling exceeds the grace period (60–120s), a worker that loses its connection will be declared dead and its build marked FAILURE before it can reconnect — even though the worker is alive.

**Fix:** Specify the backoff explicitly (e.g., initial 1s, multiplier 2, jitter ±20%, ceiling 30s). Document the invariant: `reconnect_backoff_ceiling_secs < liveness_grace_period_secs`. Verify in config validation at startup.

### M5 — `tower-sessions-sqlx-store` table initialization ordering is unspecified

The library creates its own `tower_sessions` table. This table is not in the `migrations/` directory and not managed by `sqlx::migrate!()`. The startup procedure doesn't mention it. An implementer reading only the startup steps will not know an additional table is created.

**Fix:** Add a startup step: "Initialize tower-sessions-sqlx-store; creates the `tower_sessions` table if not present." Document that this table is managed by the library, not cbsd migrations, and note potential version-upgrade concerns.

---

## Minor Issues

- **`POST /api/builds/new` stale reference.** The dispatch logic section references `POST /api/builds/new` but the README lists `POST /builds` (no `/new`). One-line fix.
- **`builds.state` CHECK allows `"new"` but builds enter `QUEUED` immediately.** If the `new` state is never persistent, remove it from the CHECK constraint. If there's a transient window, document it.
- **`key_prefix` uniqueness is per-owner, not global.** Two users can have keys with the same prefix. Admin deletion via `DELETE /auth/api-keys/{prefix}` must scope to the owner — document this.
- **`worker_stopping.worker_id` is redundant.** Server tracks by connection handle. Document that the field is for logging only to prevent identity-resolution bugs.
- **`worker_stopping` mid-dispatch race with ack timer.** If the worker is already `Stopping` when the ack timer fires, the ack-timeout path should not mark it "suspect." Add a note.
- **`component_sha256` integrity failure re-queues to another worker.** A corrupt tarball from the server will fail on all workers serially. On integrity rejection, mark the build `failure` and log a server-side alarm instead of re-queuing.
- **`DELETE /auth/api-keys/{prefix}` case sensitivity.** Prefixes are lowercase hex. Specify that matching is case-sensitive and the canonical form is lowercase.
- **`token_hash` UNIQUE constraint creates an implicit index.** Not called out explicitly unlike `idx_tokens_user`. State it for readers auditing query performance.
- **`PUT /permissions/users/{email}/roles` with empty scopes.** The request body must distinguish "include builder with empty scopes array" (rejected) from "omit builder entirely" (removed, subject to guard). Specify in the request body schema.
- **CLI localhost callback port selection.** Specify ephemeral port via `bind(0)` to avoid CI environments with restricted port ranges.
- **`serde_yaml = "0.9"` is in maintenance mode.** Consider `figment` with YAML support or accept the dependency explicitly. Note in the project structure doc.
- **`reqwest` in `cbsd-worker` may be unnecessary.** If only used for the Bearer header, `tungstenite::http::HeaderValue` can do this directly. Justify or remove.
- **`descriptor_version` column has no versioned deserialization strategy.** Document how `cbsd-server` will handle v1 blobs when the schema reaches v2.
- **`scope_type` in `user_role_scopes` has no `CHECK` constraint.** Add `CHECK (scope_type IN ('project', 'registry', 'repository'))`.
- **`glob-match` wildcard semantics.** Verify that `*` does not match path separators. If it does, `ces-devel/*` matches `ces-devel/subdir/repo`, which may not be intended.
- **DISPATCHED→STARTED latency.** After `build_accepted` cancels the ack timer, the gap to `build_started` is covered only by the 60–120s grace period. Document expected latency in the API reference so clients can set timeout alerts.

---

## Suggestions

- **Add `PRAGMA synchronous = NORMAL`** alongside WAL mode. WAL + NORMAL avoids per-checkpoint fsync while maintaining durability against OS crashes. The default FULL is unnecessary for this workload.
- **Use `#[serde(rename_all = "lowercase")]` or explicit renames** on the `Arch` enum in `cbsd-proto`. Without this, Rust's conventional `X86_64` variant serializes as `"X86_64"`, silently failing the arch validation.
- **Document the sqlx offline query cache** (`.sqlx/` dir or `sqlx-data.json`). Compile-time checked queries require this to build without a live database (CI gotcha).
- **Consider separating the cbscore result JSON line onto fd 3** instead of mixing it with build output on stdout. Eliminates per-line JSON prefix inspection. Low priority — current approach is workable.
- **Freeze the PASETO token payload schema.** The README lists this as open. It's the most important unfrozen spec — both the Python migration script and Rust server must SHA-256 the same bytes. Document field names, field order, encoding of `expires_at`, and whether `jti` is included.
- **Specify the session signing key derivation.** Document: "HKDF-SHA256 with input key = `token_secret_key` and context string `cbsd-oauth-session-v1`." Must be stable across restarts.
- **Clarify `max_token_ttl_seconds = 0` semantics.** The convention of `0` meaning "unlimited" is ambiguous. Consider `null`/`None` for "no limit" and `0` for "expire immediately."
- **Specify the `dispatched → started` expected latency range** in the API reference so clients can set meaningful timeout alerts (currently covered only by the 60–120s grace period).

---

## Strengths

- **Dispatch mutex split** — pop under lock, send outside lock, re-push on failure. Correctly avoids holding the mutex across network I/O while maintaining the invariant that a build cannot be simultaneously in a priority lane and the active map.
- **Reconnection decision table** — all 9 reconnect-state/report combinations are covered. The "always revoke unknown" invariant prevents zombie builds. This is the hardest correctness property in a distributed queue.
- **Seq-to-offset index + watch channel** — per-line granular seq enables exact SSE replay, O(log n) binary search on the index, watch-channel wakeup avoids polling overhead. Production-quality.
- **`user_role_scopes` table** — scopes on assignments, not roles. Correctly models per-user scope requirements without role proliferation. Scope validation at assignment time prevents confusing 403s.
- **REVOKING intermediate state** — 30s ack timeout separate from 60–120s liveness grace period. Late `build_output` after `finished=1` silently discarded. Closes the race cleanly.
- **Bootstrapping in single transaction** — atomic seeding: roles → admin user → role assignment → API keys. No partial-seed recovery state.
- **cbscore subprocess bridge** — process-group SIGTERM, SIGKILL escalation, structured result line on stdout. Reflects operational experience with the current Python implementation.
- **Deferring periodic builds entirely** — no stub endpoints creating API contract debt.
- **SHA-256 vs argon2 dual hashing** — correct rationale documented, prevents future second-guessing.
- **`descriptor_version` on builds table** — correct mechanism for long-lived JSON blob evolution.
- **Schema quality** — integer epoch timestamps, CHECK constraints on state enums, explicit indices, `builtin` guard on system roles.

---

## Open Questions

- **PASETO token payload schema.** Must be frozen before auth subsystem implementation. Field names, types, `expires_at` encoding, `jti` presence — both Python migration script and Rust server must hash the same bytes.
- **What string is passed to `require_scope(ScopeType::Registry, ...)`?** Full image name, registry hostname, or component repo URL? The Python system checks channel and component repo URL, not destination image registry.
- **Does `GET /components` expose too much?** Authenticated but roleless users can enumerate component names and versions. If names reflect internal project details or unreleased Ceph versions, "not sensitive" may need revisiting.
- **Is there a maximum log file size or GC policy?** The Python system GCs Redis log streams after 6 hours. The Rust design writes to disk with no specified size limit or cleanup.
- **Worker container image specifics.** Python environment version, uv/venv/system packages, how `cbscore.config.yaml` is injected — all unspecified. Near-term implementation blocker.
- **Session signing key derivation function and context string.** Must be stable across restarts and specified precisely.
- **`max_token_ttl_seconds = 0` semantics.** Unlimited or expire-immediately? Freeze before config format is declared stable.
