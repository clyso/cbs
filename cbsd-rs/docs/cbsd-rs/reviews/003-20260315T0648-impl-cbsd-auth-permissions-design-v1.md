# Implementation Review: cbsd-rs Commit 4 (Phase 2 completion)

**Commit reviewed:**


- `f98c88f` — cbsd-rs/server: add OAuth, API key management, and auth routes


**Evaluated against:**

- Plan: `cbsd-rs/docs/cbsd-rs/plans/003-20260318T1411-authentication.md` (Commit 4 section)
- Design: `cbsd-rs/docs/cbsd-rs/design/003-20260313T2129-cbsd-auth-permissions-design.md`

---

## Summary

Commit 4 is a substantial, well-implemented addition (~1200 lines across
8 new files) that delivers the complete OAuth flow, API key management with
LRU cache, and all auth route handlers. The LRU cache implementation with
three-map reverse indexes and correct eviction cleanup is particularly
strong. Two plan-specified features are missing (HKDF session key derivation
and rate limiting), and the `db/roles.rs` module was added ahead of
schedule (plan assigns it to Commit 5). One code quality issue is noted.

**Verdict: Good implementation with two missing plan items to track.**

---

## Plan Compliance

### Fulfilled

- **`auth/oauth.rs`:** Google OAuth helpers — `load_oauth_config()` from
  secrets JSON, `build_google_auth_url()` with `hd=` domain hint,
  `exchange_code_for_userinfo()` (code → token → userinfo two-step) ✓
- **`auth/api_keys.rs`:** Full LRU cache implementation:
  - `ApiKeyCache` with `by_sha256`, `by_prefix`, `by_owner` maps ✓
  - `CachedApiKey` includes `key_prefix` (design requirement for eviction) ✓
  - `insert()` handles `push()` return for LRU eviction cleanup ✓
  - `remove_by_prefix()` and `remove_by_owner()` for targeted invalidation ✓
  - `create_api_key()` with argon2 via `spawn_blocking` ✓
  - `verify_api_key()` with SHA-256 cache lookup, argon2 fallback via
    `spawn_blocking`, prefix-based DB query ✓
  - Key format: `cbsk_<64 hex>`, prefix = chars 5..17 (12 hex chars) ✓
- **`db/api_keys.rs`:** Full CRUD — insert, find by prefix, list for user,
  revoke by owner+prefix, revoke all for user ✓
- **`routes/auth.rs`:** All 8 endpoints:
  - `GET /api/auth/login` — session state (oauth_state, client_type,
    cli_port), redirect to Google ✓
  - `GET /api/auth/callback` — CSRF validation, domain restriction,
    user create/update, PASETO token, session `cycle_id()` (fixation
    prevention), CLI/web response branching ✓
  - `GET /api/auth/whoami` — returns email, name, roles, caps (stubs
    for Commit 5) ✓
  - `POST /api/auth/token/revoke` — self-revoke from bearer ✓
  - `POST /api/auth/tokens/revoke-all` — bulk revoke by email ✓
  - `POST /api/auth/api-keys` — create, returns 201 with plaintext ✓
  - `GET /api/auth/api-keys` — list own keys (prefix + name + created_at) ✓
  - `DELETE /api/auth/api-keys/{prefix}` — revoke + cache purge ✓
- **`AuthUser` extractor updated:** API key path now functional (was
  placeholder in Commit 3). Both PASETO and API key paths load user
  record, check active status, load capabilities ✓
- **Error response `{"detail": "..."}`** on all error paths ✓
- **CSP header** on CLI token display page: `default-src 'none'; script-src 'none'` ✓
- **CLI localhost redirect** via `cli_port` session parameter ✓
- **Web redirect** to `/#token=<base64>` ✓
- **AppState extended** with `oauth: OAuthState` and
  `api_key_cache: Arc<Mutex<ApiKeyCache>>` ✓
- **Plan progress table updated** ✓

### Missing

**1. HKDF session key derivation — not implemented.**

The plan specifies: "Session signing key derived from `token_secret_key`
via HKDF-SHA256 with context `cbsd-oauth-session-v1`." The `hkdf` crate
is in `Cargo.toml` but is not used anywhere. The session layer is
constructed with `tower-sessions`' default signing, which likely generates
a random key at startup. This means OAuth sessions do not survive server
restarts — any user mid-flow when the server restarts will get a CSRF
validation failure.

Severity: **Medium.** Not a correctness bug (the system works), but
sessions break on restart and the design's domain-separation requirement
between PASETO key and session key is not enforced.

**2. Rate limiting via `tower-governor` — not implemented.**

The plan specifies: "Rate limiting via `tower-governor` on login/callback
(10 req/min/IP)." No `tower-governor` dependency appears in `Cargo.toml`
and no rate limiting middleware is applied to the auth routes.

Severity: **Low for dev, medium for production.** An unauthenticated
endpoint that hits Google OAuth and creates sessions on every call should
be rate-limited before deployment.

### Ahead of Schedule

**`db/roles.rs` added in Commit 4 (plan assigns to Commit 5).**

This module includes: role CRUD, capability management, user-role
assignments with scopes, `get_effective_caps()`, `get_user_assignments_with_scopes()`,
`count_active_wildcard_holders()`, `has_assignments()`, `is_role_builtin()`.

This is a clean addition that enables the `AuthUser` extractor to load
capabilities immediately (used in the extractor's PASETO and API key
paths). The scope evaluation logic (`require_scopes_all()`) is also present
in `extractors.rs`. This is reasonable forward work — the extractor needs
caps and scopes to be useful, and leaving them as empty stubs would make
the auth routes untestable.

The `db/roles.rs` module is complete and correct for what Commit 5 needs.
The plan's Commit 5 progress table should reflect this.

---

## Code Quality

### Good

- **LRU eviction cleanup is correct.** The `insert()` method checks
  `evicted_hash != sha256` before cleaning reverse maps (handles the
  case where the same key is re-inserted). `remove_by_prefix()` and
  `remove_by_owner()` correctly clean all three maps.
- **Argon2 operations use `spawn_blocking`.** Both `create_api_key` and
  `verify_api_key` run argon2 in a blocking thread. This was a specific
  plan requirement.
- **Domain restriction is checked at callback time** (not just via `hd=`
  hint, which is client-side only). Server-side check is the real gate. ✓
- **Session ID regeneration** via `session.cycle_id()` at callback —
  prevents session fixation. ✓
- **`revoke_all_tokens` checks target user exists** before bulk revoke. ✓
- **`by_prefix` maps to `HashSet<[u8; 32]>`**, not a single hash — handles
  the (unlikely but possible) case of two keys sharing a prefix from
  different owners.
- **`scope_pattern_matches()` is simple and correct** — exact match or
  prefix-glob. The `glob-match` crate question is deferred to Commit 5.

### Issues

**Issue 1 — Duplicate user-load + active-check pattern in AuthUser extractor.**


The PASETO path (lines 205–245) and the API key path (lines 172–203) both
contain identical logic:

```rust
let user = db::users::get_user(&state.pool, &email).await...?
    .ok_or_else(...)?;
if !user.active { return Err(...); }
// load caps
let caps = db::roles::get_effective_caps(&state.pool, &user.email).await...?;
Ok(AuthUser { email: user.email, name: user.name, caps })
```

This pattern appears twice with only the email source differing
(`payload.user` for PASETO, `cached.owner_email` for API keys). It could
be extracted into a helper:

```rust
async fn load_authed_user(pool: &SqlitePool, email: &str) -> Result<AuthUser, AuthError> {
    let user = db::users::get_user(pool, email).await...?
        .ok_or_else(...)?;
    if !user.active { return Err(...); }
    let caps = db::roles::get_effective_caps(pool, &user.email).await...?;
    Ok(AuthUser { email: user.email, name: user.name, caps })
}
```

This eliminates ~20 lines of duplication and ensures the two auth paths
can't silently diverge (e.g., if a new check is added to one but not the
other).

Severity: **Low.** Correctness is not affected. This is a maintainability
improvement.

**Issue 2 — `LoginQuery` defaults to `"web"`, plan defaults to `"cli"`.**

`routes/auth.rs` line 52: `fn default_client() -> String { "web".to_string() }`

The design doc specifies: "If omitted, defaults to `cli` for backwards
compatibility." The implementation defaults to `"web"`. This is a minor
deviation — the design's rationale is that existing `cbc` clients that
don't pass the parameter should get CLI behavior by default.

Severity: **Low.** Easy one-line fix.

**Issue 3 — `by_prefix` reverse map uses `HashSet` but `by_sha256` LRU eviction doesn't clean empty `by_prefix` entries after `remove_by_owner`.**

`remove_by_owner()` (lines 109–122) removes entries from `by_sha256` and
then removes the SHA-256 from `by_prefix` sets. But when it removes an
entry from `by_sha256` via `pop()`, it gets the `CachedApiKey` which has
`key_prefix`. It then uses that to clean `by_prefix`. This is correct.

However, the `remove_by_prefix()` method (lines 93–106) removes the
entire prefix entry from `by_prefix`, then cleans `by_owner`. If a prefix
key was already removed from `by_sha256` by a prior eviction (stale entry
in `by_prefix`), the `self.by_sha256.pop(h)` returns `None` and the
`by_owner` cleanup is skipped for that hash. This leaves a stale hash in
`by_owner` — harmless but technically a memory leak that grows with
evictions.

Severity: **Negligible.** The `by_owner` set would contain hashes that no
longer exist in `by_sha256`. A subsequent `remove_by_owner` call would
try to pop those from `by_sha256` (cache miss), silently skip them, and
then remove them from `by_owner`. The stale entries are self-cleaning on
the next owner-level operation.

---

## Design Fidelity

| Design requirement | Status | Notes |
|---|---|---|
| Google OAuth with `hd=` domain hint | ✓ | Single-domain hint |
| Domain restriction at callback | ✓ | Server-side check |
| `allowed_domains` / `allow_any_google_account` | ✓ | Validated at startup |
| Session fixation prevention (`cycle_id`) | ✓ | |
| CLI token paste page with CSP | ✓ | `default-src 'none'; script-src 'none'` |
| CLI localhost redirect via `cli_port` | ✓ | JS redirect in HTML |
| Web redirect to `/#token=<base64>` | ✓ | |
| API key format `cbsk_<64 hex>` | ✓ | 32 random bytes |
| API key prefix: 12 chars after `cbsk_` | ✓ | chars 5..17 |
| Argon2 for API key hashing | ✓ | Via `spawn_blocking` |
| LRU cache with reverse indexes | ✓ | `by_sha256`, `by_prefix`, `by_owner` |
| `CachedApiKey` includes `key_prefix` | ✓ | |
| Eviction cleanup via `push()` return | ✓ | |
| `Arc<Mutex<ApiKeyCache>>` | ✓ | |
| Error response `{"detail": "..."}` | ✓ | All endpoints |
| Self-revoke via bearer token | ✓ | PASETO only, API keys directed to DELETE |
| Bulk token revocation | ✓ | `permissions:manage` check deferred to Commit 5 |
| HKDF session key derivation | ✗ | Crate present, code not written |
| Rate limiting on login/callback | ✗ | Not implemented |
| `whoami` response shape | Partial | Roles/caps populated, full scope detail in Commit 5 |

---


## Bonus: Ahead-of-Schedule Work

`db/roles.rs` (374 lines) is a complete RBAC database layer including:

- Role CRUD with builtin protection
- Capability management (`set_role_caps`, `get_role_caps`)
- User-role assignments with per-assignment scopes
- `get_effective_caps()` — deduplicated union across roles
- `get_user_assignments_with_scopes()` — for scope evaluation

- `count_active_wildcard_holders()` — for last-admin guard
- `has_assignments()`, `is_role_builtin()` — helpers

`extractors.rs` additions (also ahead of Commit 5):

- `ScopeType` enum with `as_str()` for DB matching
- `has_cap()`, `has_any_cap()` — capability checks with `*` wildcard
- `require_scopes_all()` — assignment-level AND semantics with glob matching
- `scope_pattern_matches()` — exact or prefix-glob

This work is correctly placed — the extractor needs it to be functional.
