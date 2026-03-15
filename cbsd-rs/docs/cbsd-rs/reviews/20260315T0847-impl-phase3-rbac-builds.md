# Implementation Review: cbsd-rs Phase 2 Fixes + Phase 3

**Commits reviewed:**
- `c76962a` — Commit 4a: Google OAuth flow with HKDF session signing
- `2ce89c4` — Commit 4b: API key management with LRU cache and auth routes
- `d6beaca` — Commit 5a: RBAC database layer and scope evaluation
- `988bfdc` — Commit 5b: Permission and admin route handlers
- `0f6edf7` — Commit 6: Build queue, submission, listing, and components

**Note:** Commits 4 and 5 were originally single commits that were split
into two each per the granularity assessment. This review covers the
split versions. Commit 4 fixes (HKDF, rate limiting, `load_authed_user`,
default `"cli"`) were fixup'd into the split commits.

**Evaluated against:**
- Plan: `_docs/cbsd-rs/plans/phase-2-authentication.md` (Commit 4)
- Plan: `_docs/cbsd-rs/plans/phase-3-permissions-builds.md` (Commits 5–6)
- Design: `_docs/cbsd-rs/design/2026-03-13-cbsd-auth-permissions-design.md`

---

## Summary

Phases 2 (completion) and 3 are well-implemented across 5 commits totaling
~3726 authored lines. The commit splits follow the recommended boundaries
exactly — OAuth / API keys and DB+extractor / route handlers. All prior
review fixes are incorporated. The last-admin guard is implemented across
all 5 mutation paths with correct transactional semantics. One design
deviation is noted (`role_is_scope_dependent` incorrectly treats `*` as
scope-dependent).

**Verdict: Good implementation. Proceed to Phase 4.**

---

## Commit 4a (`c76962a`) — Google OAuth Flow (528 lines)

### Plan Compliance: Complete for OAuth scope

- `auth/oauth.rs` (202 lines): Load secrets JSON, build Google auth URL
  with `hd=` domain hint, exchange code for userinfo (two-step) ✓
- Login/callback route handlers in `routes/auth.rs` (267 lines) ✓
- HKDF session key derivation in `main.rs`: `Hkdf::<Sha256>::new(None,
  token_key_bytes).expand(b"cbsd-oauth-session-v1", ...)` → `.with_signed()` ✓
- `tower-governor` rate limiting on `/login` and `/callback` (10/60s/IP) ✓
- Session state: `oauth_state` + `client_type` + `cli_port` ✓
- Domain restriction at callback (server-side check) ✓
- Session `cycle_id()` for fixation prevention ✓
- CLI: token paste page with CSP `default-src 'none'; script-src 'none'` ✓
- CLI: localhost redirect via `cli_port` ✓
- Web: redirect to `/#token=<base64>` ✓
- `LoginQuery` default: `"cli"` (per design doc) ✓
- API key extractor path remains placeholder ✓

---

## Commit 4b (`2ce89c4`) — API Key Management (739 lines)

### Plan Compliance: Complete for API key scope

- `auth/api_keys.rs` (309 lines): `ApiKeyCache` with `by_sha256`,
  `by_prefix`, `by_owner` reverse maps. `CachedApiKey` includes
  `key_prefix`. LRU eviction cleanup via `push()` return value.
  `create_api_key()` and `verify_api_key()` with argon2 via
  `spawn_blocking` ✓
- `db/api_keys.rs` (142 lines): insert, find by prefix, list for user,
  revoke by owner+prefix, revoke all for user ✓
- Extractor API key path wired up (`cbsk_` prefix → `verify_api_key`) ✓
- Auth route handlers: whoami, token revoke (self), bulk token revoke,
  API key create/list/revoke ✓
- `AppState` extended with `api_key_cache: Arc<Mutex<ApiKeyCache>>` ✓
- `load_authed_user()` helper shared between PASETO and API key paths ✓
- Phase 2 plan progress table updated ✓

---

## Commit 5a (`d6beaca`) — RBAC Database Layer + Scope Evaluation (470 lines)

### Plan Compliance: Complete for DB+extractor scope

Clean 3-file commit:

- `db/roles.rs` (373 lines): 15 functions for role/cap/scope CRUD
  including `get_effective_caps()`, `get_user_assignments_with_scopes()`,
  `count_active_wildcard_holders()`, `set_user_roles()` in single
  transaction ✓
- `auth/extractors.rs` (+96 lines): `ScopeType` enum, `has_cap()` with
  `*` wildcard, `has_any_cap()`, `require_scopes_all()` with
  assignment-level AND semantics, `scope_pattern_matches()` ✓
- `db/mod.rs` (+1 line): `pub mod roles` ✓

No route handlers — independently testable DB + evaluation logic.

---

## Commit 5b (`988bfdc`) — Permission and Admin Route Handlers (1026 lines)

### Plan Compliance: Complete

**`routes/permissions.rs`** (812 lines):
- All 10 endpoints implemented ✓
- `KNOWN_CAPS` validation on create/update (400 for unknown) ✓
- `SCOPE_DEPENDENT_CAPS` enforcement on assignment (400 if missing) ✓
- `?force=true` for cascade deletion ✓
- Builtin role protection (409 on delete/modify) ✓

**`routes/admin.rs`** (188 lines):
- Deactivation: transactional with in-transaction last-admin guard ✓
- Idempotent (already-inactive → 200, skips guard) ✓
- Bulk revoke tokens + API keys after commit ✓
- LRU cache purge via `remove_by_owner` ✓
- Activation: idempotent, no credential restore ✓

**Last-admin guard — all 5 mutation paths:**

| Path | Implementation | Correct |
|------|---------------|---------|
| `PUT /permissions/users/{email}/roles` (replace) | `last_admin_guard()` after `set_user_roles()` | ✓ |
| `DELETE /permissions/users/{email}/roles/{role}` | `last_admin_guard()` after `remove_user_role()` | ✓ |
| `PUT /admin/users/{email}/deactivate` | In-transaction guard (count query inside tx) | ✓ |
| `DELETE /permissions/roles/{name}` | Guard after delete if role had `*` | ✓* |
| `PUT /permissions/roles/{name}` (cap update) | Guard after `set_role_caps()` if `*` removed, with rollback | ✓ |

*\*Role deletion guard runs after CASCADE — known limitation, accepted in
design review.*

**`whoami` response updated:** Returns actual roles and effective caps. ✓
Phase 3 plan progress table updated. ✓

---

## Commit 6 (`0f6edf7`) — Build Queue + Submission (963 lines)

### Plan Compliance: Complete

- `queue/mod.rs` (230 lines): 3-lane `BuildQueue` + `SharedBuildQueue`,
  `enqueue()`, `enqueue_front()`, `next_pending()`, `remove_by_id()`,
  `pending_counts()`, `contains()`, 6 unit tests ✓
- `db/builds.rs` (164 lines): `insert_build()`, `get_build()`,
  `list_builds()`, `update_build_state()`, `insert_build_log_row()`,
  `row_to_build_record()` helper ✓
- `routes/builds.rs` (364 lines): submit, list, get, revoke (QUEUED only),
  log stubs ✓
- `components/mod.rs` (90 lines): filesystem scan, `cbs.component.yaml`,
  `validate_component_name()` ✓
- `routes/components.rs` (40 lines): `GET /api/components` ✓
- `routes/admin.rs` extended: `GET /api/admin/queue` from in-memory state ✓
- `AppState` extended: `queue`, `components` ✓
- Phase 3 + README plan progress updated ✓

---

## Code Quality & Issues

### Issue — `role_is_scope_dependent` treats `*` as scope-dependent

`routes/permissions.rs` line 153:
```rust
fn role_is_scope_dependent(caps: &[String]) -> bool {
    caps.iter().any(|c| SCOPE_DEPENDENT_CAPS.contains(&c.as_str()) || c == "*")
}
```

The design says: "Roles with `*` capability (admin) need no scopes — they
are global by definition." A custom role with `["*"]` cannot be assigned
without scopes due to this check.

Severity: **Medium.** Fix: remove `|| c == "*"`.

### Observation — QUEUED revocation not atomic (Phase 3 only)

Queue mutex released before DB state update. Benign in Phase 3 (no
dispatch). Should be addressed when dispatch is wired in (Phase 4).

### Observation — `list_users_with_roles` N+1 queries

Negligible at current scale (~10 users).

### Observation — Components loaded at startup, no reload

Acceptable for v1. Adding/removing components requires server restart.

### Observation — `update_build_state` COALESCE semantics

`COALESCE(?, error)` means NULL input preserves existing error. Cannot
explicitly clear an error once set. Fine for v1 (errors only on terminal
states).

---

## Design Fidelity Summary

| Design requirement | Status | Commit |
|---|---|---|
| HKDF session key derivation | ✓ | 4a |
| Rate limiting on login/callback | ✓ | 4a |
| Domain restriction at callback | ✓ | 4a |
| Session fixation prevention | ✓ | 4a |
| CSP on token paste page | ✓ | 4a |
| API key LRU cache with reverse indexes | ✓ | 4b |
| Argon2 via spawn_blocking | ✓ | 4b |
| Error response `{"detail": "..."}` | ✓ | 4a/4b |
| Known caps validation (400 for unknown) | ✓ | 5b |
| Scope-dependent role assignment rejection | ✓* | 5b |
| Builtin role protection (409) | ✓ | 5b |
| Last-admin guard: all 5 paths | ✓ | 5b |
| Deactivation: idempotent, bulk revoke, LRU purge | ✓ | 5b |
| Build queue: 3 lanes, SharedBuildQueue | ✓ | 6 |
| Build submission: component + scope validation | ✓ | 6 |
| Build submission: signed_off_by overwrite | ✓ | 6 |
| Build listing: :own/:any with implicit filtering | ✓ | 6 |
| Build get: :own ownership check | ✓ | 6 |
| QUEUED revocation: remove + mark REVOKED | ✓ | 6 |
| Component listing: auth-only, no cap required | ✓ | 6 |
| Admin queue: reads in-memory state | ✓ | 6 |
| `insert_build_log_row()` defined for Phase 4 | ✓ | 6 |

*\*`role_is_scope_dependent` incorrectly treats `*` as scope-dependent.*
