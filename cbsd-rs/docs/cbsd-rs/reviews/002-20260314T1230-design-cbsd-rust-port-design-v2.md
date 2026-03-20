# Design Review: cbsd Rust Port (Post-Revision)

**Documents reviewed:**
- `_docs/cbsd-rs/README.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-rust-port-design.md`
- `_docs/cbsd-rs/2026-03-13-cbsd-auth-permissions-design.md`

---

## Summary

The design has matured substantially since the prior review cycle. All previously identified blockers have been resolved: the dispatch ack timeout, REVOKING intermediate state, bootstrapping sequence, seq-to-offset index, split-mutex dispatch, per-assignment scopes, and periodic builds deferral are all correctly addressed. The architecture is sound and the migration plan is explicit.

Three new blockers have been identified in this pass — none requiring architectural rethink. The most critical is B1: the SSE `seq` field is batch-granular (one seq per up to 50 lines) but SSE events are emitted per-line, making mid-batch reconnect produce duplicate log lines. B2 (any Google account can authenticate, no domain restriction) and B3 (argon2 per-request cost for API-key REST callers) round out the blockers. Additionally, the last-admin guard has two additional bypass paths beyond the deactivation gap already identified: custom role deletion via CASCADE and admin role capability modification. All findings have clear, bounded resolution paths.

**Verdict: Revise and re-review.** One more focused pass should bring this to implementation readiness.

---

## Blockers

Issues that must be resolved before implementation begins.

### B1 — SSE `seq` is batch-granular; per-line resume produces duplicate log lines

The `build_output` WebSocket message carries a single `seq` field per batch (up to 50 lines). The SSE log-follow endpoint emits one SSE event per log line, each with `id: <seq>`. When a client disconnects mid-batch (after receiving, say, line 20 of a 50-line batch at seq=7) and reconnects with `Last-Event-ID: 7`, the server re-sends all 50 lines from seq=7 — duplicating the first 20 the client already received.

This will be visibly broken in any CLI or browser UI following an active build whenever the connection is interrupted mid-batch.

**Fix (choose one):**
1. **Per-line seq (recommended).** `build_output` carries `start_seq` and `lines`. Individual seq values are `start_seq`, `start_seq+1`, ..., `start_seq+len-1`. Each SSE event has `id: <per_line_seq>`. Resume is exact.
2. **Composite SSE event IDs.** Format: `<seq>:<line_index>` (e.g., `7:20`). Server parses both on resume. More complex but avoids WS protocol changes.
3. **Accept at-least-once delivery.** Document that mid-batch reconnects produce duplicates. Clients must deduplicate. Lowest effort but weakens SSE's correctness guarantee.

### B2 — Any Google account can authenticate; no domain restriction

The OAuth callback unconditionally creates a user record and issues a PASETO token for any Google account that completes the flow. There is no `hd=` hosted-domain restriction, no email allowlist, and no domain filter in the config schema. A personal Gmail account or any third-party Google workspace account can authenticate, receive a token, and enumerate component names via `GET /components`.

**Fix:** Add `allowed_domains: ["clyso.com"]` to server config. At callback, after extracting the email but before creating the user record, verify the email domain is allowed. Reject with HTTP 403 if not. If open access is genuinely intended, require an explicit `allow_any_google_account: true` config to make this a conscious decision. Also add `hd=` to the Google authorization URL for defense-in-depth (server-side check is the real gate).

### B3 — Argon2 API key verification is O(100–500ms) per request for non-worker callers

The design says "API key validation is infrequent (connection setup, not per-request)" to justify argon2. This is true for workers (single WebSocket upgrade), but the design also supports API keys for human self-service (`apikeys:create:own`) and CI pipelines. Any `cbc` invocation or CI script using an API key for REST calls (build submission, status polling, log tailing) pays argon2 on every HTTP request — 100–500ms of auth overhead per call with no caching mechanism.

**Fix:** Add an in-memory LRU cache for verified API keys. Cache key: SHA-256 of the raw API key string (cheap, doesn't expose the key). Cache value: `{owner_email, roles, expires_at}`. Cache entries bounded (e.g., 512, LRU eviction). Entries purged on explicit revocation. Subsequent requests pay only a SHA-256 + LRU lookup — sub-microsecond. Specify this in the design so the implementation plans for it.

---

## Major Concerns

Significant issues that will cause pain if not addressed.

### M1 — Last-admin guard has three unguarded mutation paths

The guard is specified on `PUT /api/permissions/users/{email}/roles`. Three additional paths bypass it:

1. **User deactivation.** Setting `users.active = 0` on the sole active admin leaves no active admin. The role assignment is unchanged, so the guard doesn't fire.
2. **Custom role deletion.** A custom role with `*` capability is not built-in. `DELETE /permissions/roles/{name}` triggers `ON DELETE CASCADE` on `user_roles`, silently removing the last admin assignment if that was the only role granting `*`.
3. **Admin role capability modification.** `PUT /permissions/roles/admin` can remove `*` from the built-in admin role's capabilities. The built-in flag prevents deletion but not capability modification.

**Fix:** The same invariant ("at least one active user retains `*` capability") must be checked in all four mutation paths: user-role replacement, user-role removal, user deactivation, role deletion, and role capability update. Any operation that would violate the invariant returns HTTP 409.

### M2 — `:own` capability enforcement semantics are never specified

`DELETE /builds/{id}` requires `builds:revoke:own` OR `builds:revoke:any`. The `RequireCap` extractor verifies capability presence but cannot check resource ownership. The handler-level ownership check (`builds.user_email == authenticated_user.email`) is never described. Without it, `builds:revoke:own` and `builds:revoke:any` are equivalent in practice — a privilege escalation.

Same gap applies to `GET /builds/{id}` with `builds:list:own`: the README describes implicit scope filtering for the list endpoint but not for single-resource routes.

**Fix:** Add a section to the auth design specifying the enforcement pattern for `:own`/`:any` capability pairs: "For endpoints accepting a resource ID and supporting both variants, the handler must: (1) load the resource, (2) if caller has only `:own`, verify `resource.owner == authenticated_user.email` and return 403 if mismatch, (3) if caller has `:any`, skip the ownership check."

### M3 — Worker API key in URL query string contradicts Bearer auth and logs credentials

The arch doc specifies `wss://<server>/api/ws/worker?token=<api-key>`. The auth doc specifies `Authorization: Bearer <key>`. These contradict each other. The query-string approach logs the raw API key in every HTTP access log, reverse-proxy log, and TLS terminator log.

**Fix:** Remove `?token=<api-key>` from the arch doc. Use `Authorization: Bearer <api-key>` consistently — axum supports reading headers in the HTTP upgrade handler. If a query-parameter fallback is needed for specific client libraries, document it as opt-in with an explicit warning about credential logging.

### M4 — Dispatch ack timer lifecycle incomplete; `build_accepted` → `build_started` gap unguarded

What happens when `build_accepted` arrives? The ack timer should cancel — this is not stated. A worker that sends `build_accepted` then crashes before `build_started` leaves the build in DISPATCHED with no timeout governing the transition. The build is stuck until the WebSocket drops (60–120s grace period).

**Fix:** Explicitly state that `build_accepted` cancels the ack timer. Document how the DISPATCHED → STARTED gap is covered: "If the worker sends `build_accepted` but disconnects before `build_started`, the grace period handles it — the build is marked FAILURE when the worker is declared dead. This is intentional; we rely on liveness detection, not a dedicated `build_started` timeout."

---

## Minor Issues

- **`POST /auth/token/revoke` request body is unspecified.** What identifies the token to revoke — raw token string, hash, or internal ID? For self-revocation, infer from the bearer token. For revoking another user's token, the body must identify it. Define the request schema.
- **`tokio::sync::watch` semantics need clarification.** `watch` is single-slot; intermediate notifications are coalesced. The SSE handler must read from current file position to EOF on wakeup, not just to the offset in the watch value. Document that `watch` is a wakeup signal, not a per-batch delivery mechanism.
- **`scope_type` in `user_role_scopes` has no `CHECK` constraint.** Add `CHECK (scope_type IN ('project', 'registry', 'repository'))` to match the `builds.state` pattern.
- **OR-capability pattern not expressible with `RequireCap<C>`.** Endpoints requiring `builds:revoke:own` OR `builds:revoke:any` need either a multi-cap extractor variant or an `AuthUser` extraction with manual OR check. Specify the implementation pattern.
- **`cbsk_` prefix included in 12-character `key_prefix`.** If the 12 chars include `cbsk_` (5 chars), only 7 random chars are captured — 28 bits of prefix space. Specify that `key_prefix` captures the first 12 chars of the random portion (post-`cbsk_`), giving 48 bits.
- **Localhost OAuth callback requires pre-registered redirect URI.** Google requires all redirect URIs registered in advance. A random port can't be registered. Use a fixed port or the `http://localhost` pattern (port-agnostic). Note this as a `cbc` implementation constraint.
- **`GET /builds/{id}/logs/tail?n=30` has no maximum `n`.** An unbounded `n` causes the server to read an entire multi-megabyte log into memory. Specify a cap (e.g., `n <= 10000`) and return 400 for values exceeding it.
- **User deactivation API absent from REST API surface table.** The auth doc describes it in prose but no endpoint appears in the README. Add it with required capability and specify whether deactivation also bulk-revokes tokens.
- **`PUT /permissions/roles/admin` can strip `*` from built-in admin role.** `builtin = 1` prevents deletion but not capability modification. Either prevent cap modification on builtins (409) or run the last-admin guard after any role cap update that removes `*`.
- **No `PRAGMA busy_timeout` specified.** Default is 0ms (fail immediately on write contention). Add `PRAGMA busy_timeout = 5000` alongside WAL in connection setup.
- **`POST /auth/token/revoke` vs `DELETE /auth/api-keys/{prefix}` naming inconsistency.** Both invalidate a credential. Document the rationale for different HTTP verbs or unify.
- **Dispatch step 8 conflates `build_accepted` and `build_started`.** Split into 8a (ack timer cancels, build remains DISPATCHED) and 8b (transition to STARTED).
- **Scope-dependent roles with zero scopes on `PUT` replacement.** A PUT that keeps a scope-dependent role but removes all scopes silently makes those capabilities unreachable. Consider warning when scope-dependent assignments would be left scopeless.

---

## Suggestions

- **Freeze the PASETO token payload schema** before implementation. Field names, types, whether `exp` is epoch or ISO-8601, whether `jti` is included. Record as a versioned constant shared between Python `cbsdcore` and Rust `cbsd`.
- **Decide `max_token_ttl_seconds` now.** The marginal cost is one config field and two lines of logic. Either add it or explicitly defer.
- **Document `arch` enum values as a protocol constant.** A single "Arch enum" section prevents the `"arm64"` vs `"aarch64"` mismatch the design warns about and makes future additions (`riscv64`) explicit.
- **Specify `POST /builds` request body field names.** The scope check example references `body.project` and `body.descriptor.dst_image.name` but no schema is defined.
- **Plan sqlx migration mechanism.** `sqlx::migrate!()` at startup with embedded `.sql` files is standard. Specify that backward-incompatible schema changes require coordinated deploys.

---

## Strengths

- **Split-mutex dispatch** — acquire lock, pop + insert, release lock, send WS, re-acquire on failure. Correctly avoids holding the lock across I/O.
- **Reconnection decision table** — all 9 state/report combinations covered, including the previously missing DISPATCHED + idle row. "Always revoke unknown" invariant is explicitly justified.
- **`user_role_scopes` table** — scopes on assignments, not roles. Same `builder` role assigned to different users with different scopes. Examples make it concrete and auditable.
- **Seq-to-offset index + watch channel** — O(1) seek on SSE reconnect, graceful linear-scan fallback for completed builds. Index dropped at terminal state to bound memory.
- **Bootstrapping in single transaction** — roles → admin user → role assignment → API keys. Atomic: fully seeded or not at all.
- **REVOKING intermediate state** — separate ack timeout (30s) from liveness grace period (60–120s). Late `build_output` after `finished=1` silently discarded.
- **`admin:queue:view` at `/admin/` prefix** — avoids axum routing collision, intentionally unscoped, explicitly admin-only.
- **`descriptor_version` on builds table** — correct mechanism for future schema evolution without breaking historical records.
- **`Stopping` worker state** — intentional shutdown skips grace period, immediate deregister. Mid-dispatch race covered by ack timeout.
- **Component SHA-256 integrity check** — worker verifies hash before unpacking. Rejection surfaces the problem immediately.
- **Glob patterns over regex** — eliminates quadruple-escaped YAML maintenance hazard.

---

## Open Questions

- **Which `watch` semantics does the SSE handler use?** Seek to exact offset from watch value, or read from current EOF? Determines whether `watch` or `broadcast` is the right channel type.
- **What does `POST /auth/token/revoke` accept?** Self-revocation (inferred from bearer) or explicit target? If explicit, what identifies the token?
- **Does the last-admin guard also apply to `DELETE /permissions/users/{email}/roles/{role}`?** Only `PUT` (replace-all) is mentioned.
- **Is `cbsd api-keys create --name worker-01 --db cbsd.db` (offline CLI) in scope for v1?** Useful for disaster recovery and initial deployment automation.
- **Is the Google OAuth app configured for `http://localhost` redirect URI?** Must be confirmed before implementing the v1 CLI localhost callback flow.
- **Does user deactivation bulk-revoke tokens?** Or does the extractor simply reject requests from inactive users without modifying the `tokens` table?
- **What is `DELETE /permissions/roles/{name}` behavior when assignments exist?** Silent CASCADE, or require `?force=true`?
- **What is the planned sqlx migration mechanism?** Embedded `.sql` files with `sqlx::migrate!()` at startup? Needs operational specification.
