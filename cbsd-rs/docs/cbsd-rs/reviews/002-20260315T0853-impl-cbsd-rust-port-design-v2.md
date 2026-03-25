# Implementation Review: Commit Granularity Assessment

**Scope:** All commits on the `wip/ceph-debug-component` branch since
`main`, evaluated for whether commits exceeding ~800 authored lines of
code could have been separate logical, incremental commits.

**Update (2026-03-15T0900):** Commits 4 and 5 have been split per the
recommendations below. The new 9-commit history has been verified —
all split commits compile and the boundaries are clean.

---

## Summary

The original 7-commit history had two commits that warranted splitting:
Commit 4 (OAuth + API keys, ~1220 authored lines) and Commit 5
(RBAC + permissions, ~1496 authored lines). Both have now been split
into two commits each, producing a 9-commit history where no commit
exceeds ~1026 authored lines and the two largest (Commits 5b at 1026
and 6 at 963) are at acceptable thresholds given their cohesive scope.

**Current commit history (9 commits, all verified):**

| # | Hash | Authored LOC | Subject |
|---|------|-------------|---------|
| 0 | `331fdc5` | 1079 (docs) | add project directory with CLAUDE.md |
| 1 | `ff4b4b5` | 844 | add cbsd-proto crate with shared types |
| 2 | `ec3056b` | ~550 (+Cargo.lock) | add SQLite schema, server scaffold, and config loading |
| 3 | `7b80b2e` | ~560 (+Cargo.lock) | add PASETO tokens, user/token DB, and AuthUser extractor |
| 4a | `c76962a` | 528 | add Google OAuth flow with HKDF session signing |
| 4b | `2ce89c4` | 739 | add API key management with LRU cache and auth routes |
| 5a | `d6beaca` | 470 | add RBAC database layer and scope evaluation |
| 5b | `988bfdc` | 1026 | add permission and admin route handlers |
| 6 | `0f6edf7` | 963 | add build queue, submission, listing, and components |

---

## Split Verification

### Commit 4a (`c76962a`) — Google OAuth flow (528 lines)

Contains: `oauth.rs` (202), login/callback route handlers (267),
HKDF session key derivation in `main.rs` (+23), `tower-governor` rate
limiting on login/callback, `routes/mod.rs`, `app.rs` AppState + router
updates.

- API key extractor path remains placeholder ("not yet implemented") ✓
- OAuth flow is self-contained and testable ✓
- No API key code present ✓

### Commit 4b (`2ce89c4`) — API key management (739 lines)

Contains: `api_keys.rs` (309), `db/api_keys.rs` (142), extractor API key
path wired up (+32), whoami/revoke/api-key route handlers added to
`routes/auth.rs` (244), `AppState` extended with `api_key_cache`.

- Depends on 4a (OAuth routes exist, extractor exists) ✓
- Plan progress table updated (Phase 2 complete) ✓
- API key create/verify/revoke cycle is testable independently ✓

### Commit 5a (`d6beaca`) — RBAC database layer + scope evaluation (470 lines)

Contains: `db/roles.rs` (373), extractor extensions (+96) — `ScopeType`
enum, `has_cap()`, `has_any_cap()`, `require_scopes_all()`,
`scope_pattern_matches()`.

- No route handlers ✓
- DB layer and scope evaluation logic are independently testable ✓
- Clean 3-file commit ✓

### Commit 5b (`988bfdc`) — Permission and admin route handlers (1026 lines)

Contains: `routes/permissions.rs` (812), `routes/admin.rs` (188),
`whoami` response updated with real roles/caps (+20), `routes/mod.rs`,
`app.rs` router wiring.

- Depends on 5a (DB layer + extractor logic) ✓
- All 10 permission endpoints + deactivate/activate ✓
- Last-admin guard across all 5 mutation paths ✓
- Plan progress table updated ✓

---

## Per-Commit Assessment (unchanged items)

### Commit 0 — Project directory with CLAUDE.md (1079 lines)

All documentation. Single conceptual unit.
**No split needed.**

### Commit 1 — cbsd-proto crate (844 lines)

Types are interdependent (`ws.rs` references `build.rs` types).
**No split needed.**

### Commit 2 — Schema, server scaffold, config (~550 authored + Cargo.lock)

Config + schema + pool + health endpoint form one testable unit.
**No split needed.**

### Commit 3 — PASETO, user/token DB, AuthUser extractor (~560 authored + Cargo.lock)

Tightly coupled pipeline: extractor → decode → revocation check → user load.
**No split needed.**

### Commit 6 — Build queue, submission, listing, components (963 lines)

Coherent scope with 113 lines of well-placed unit tests. Borderline but
acceptable.
**No split needed.**

---

## Final Assessment

The 9-commit history is well-structured. Each commit has a clear scope,
compiles independently, and is testable at its boundary. The Commit 4 and
5 splits follow the recommended boundaries exactly:


- 4: OAuth flow / API key management
- 5: DB+extractor foundation / route handlers

No further splits are needed for the existing commits. For future phases,
the already-planned Commit 8a/8b split for the dispatch engine should
maintain this discipline.
