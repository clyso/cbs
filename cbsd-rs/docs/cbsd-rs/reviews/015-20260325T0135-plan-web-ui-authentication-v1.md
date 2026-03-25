# Plan Review: Web UI Authentication (v1)

**Plan:** `015-20260325T0129-web-ui-authentication.md`
**Design:** `015-20260325T0040-web-ui-authentication.md`
**Reviewer:** Claude (automated)
**Date:** 2026-03-25

## Verdict: Needs Revision

The plan's technical content is correct and well-specified. The
commit boundary is wrong: commit 1 introduces a constraint for a
feature that doesn't land until commit 2, and both commits are
undersized. Should be a single commit.

---

## Findings

### F1 (High) — TTL validation in commit 1 is premature

Commit 1 adds a startup guard:

```rust
if self.secrets.max_token_ttl_seconds < WEB_SESSION_IDLE_SECS { panic!(...) }
```

This constraint exists because the design's 7-day session idle
timeout requires `max_token_ttl_seconds ≥ 604800`. But commit 1
doesn't introduce 7-day sessions — those arrive in commit 2. Until
commit 2 lands, the only sessions are 10-minute OAuth flow sessions.

**Impact:** An operator who currently runs with
`max_token_ttl_seconds = 3600` (short-lived tokens) would be blocked
at startup after commit 1, for a constraint that serves no purpose
until commit 2 exists.

This fails the git-commits smell test: "What can an operator DO after
this commit that they couldn't do before?" Answer: nothing — the
constraint is infrastructure for the next commit.

**Recommendation:** Move the TTL validation to commit 2 (or merge
into a single commit — see F2).

---

### F2 (High) — Both commits are undersized; should be one commit

| Commit | Authored lines | Guideline |
|--------|---------------|-----------|
| Commit 1 | ~30 | Well below 200-line minimum |
| Commit 2 | ~150 (net ~80 after removals) | Below 200-line minimum |
| **Combined** | **~180** | Acceptable for a focused feature |

The git-commits skill says: "Below ~200 lines: question whether the
commit stands alone as a meaningful change."

Commit 1 at ~30 lines is a config tweak. While cookie hardening is
independently valuable, the TTL validation is not (see F1). Commit 2
at ~150 lines is the actual feature. Combined at ~180 authored lines,
this is a clean single commit: "BFF session cookie auth with
explicit cookie config, TTL validation, extractor fallback, callback
rewrite, and logout."

The planner's stated reason for the split (lines 86–90): "All three
pieces are tightly coupled." Correct — and this argument applies
equally to the config changes. The cookie hardening and TTL
validation are motivated by and inseparable from the BFF feature.

**Recommendation:** Merge into one commit. It passes all five smell
tests:

1. **One-sentence purpose:** Add BFF session cookie authentication
   for web UI users.
2. **Previous commit compiles:** Yes.
3. **Revertable:** Reverting removes the entire BFF feature cleanly.
4. **Testable:** Web login, CLI login, logout, bearer auth unchanged.
5. **No dead code:** Every change has immediate callers/consumers.

---

### F3 (Low) — Logout endpoint routing not specified

The plan says `.route("/logout", post(logout))` but doesn't specify
which router group. Current auth routes are split into:

- `oauth_routes`: `/login`, `/callback` — rate-limited via governor
- `auth_routes`: `/whoami`, `/token/revoke`, etc. — no rate limiting

The logout handler doesn't use `AuthUser` (reads session directly),
so `auth_routes` is semantically wrong. It should go in
`oauth_routes` (rate-limited) to prevent abuse, or in a third group.

**Recommendation:** Specify routing. Suggest `oauth_routes` for rate
limiting.

---

### F4 (Low) — `POST /api/auth/token/revoke` gives misleading error for cookie-authenticated users

After this change, a web user authenticated via session cookie can
call `POST /api/auth/token/revoke`. The `AuthUser` extractor
succeeds (via cookie), but the handler then tries to read the
`Authorization` header (`auth.rs:400-404`), which is absent. Result:
401 "missing bearer token" after successful authentication.

Not a real bug — web users use `/logout`, not `/token/revoke`. But
the error is confusing. The handler could check the auth source and
return 400 "use /api/auth/logout for web sessions" instead.

**Recommendation:** Note this as a follow-up improvement, or add a
guard in this commit.

---

### F5 (Low) — Import cleanup needs precision

The plan says (lines 203–205):

> remove `use axum::response::Html;` and `use axum::http::HeaderValue;`

These are part of combined import lines:

```rust
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
```

`HeaderMap` is still used by the `revoke_token` handler
(`auth.rs:397`), so the first line becomes
`use axum::http::{HeaderMap, StatusCode};`. Only `Html` and
`HeaderValue` are removed.

**Recommendation:** Minor — just be precise during implementation.

---

## Verified Claims

| Claim | Status | Evidence |
|-------|--------|----------|
| `session.insert().await` is async | Correct | `routes/auth.rs:152-158` — existing usage |
| `session.get().await` is async | Correct | `routes/auth.rs:208` — existing usage |
| `session.cycle_id().await` is async | Correct | `routes/auth.rs:315` — existing usage |
| `Session::from_request_parts` available in axum extractors | Correct | `Session` is `FromRequestParts` per tower-sessions 0.14 |
| `session.set_expiry()` is sync (no `.await` in plan) | Plausible | Not used in current code; consistent with tower-sessions 0.14 API for in-memory operations |
| `session.flush().await` is async | Plausible | Not used in current code; consistent with store-interaction operations being async |
| Code to remove: `cli_port` field, session insert/read, HTML branches | Correct | `auth.rs:94`, `auth.rs:166-171`, `auth.rs:234-237`, `auth.rs:325-352` |
| `Html` and `HeaderValue` become unused after removal | Correct, with caveat | `Html` used only in callback; `HeaderValue` used only for CSP. But `HeaderMap` still used in `revoke_token` (auth.rs:397) — cannot remove |
| Extractor fallback: `Session` creates empty session when no cookie | Correct | tower-sessions behavior — `Session::from_request_parts` always succeeds |
| `paseto::token_hash()` works without PASETO decode | Correct | `auth/paseto.rs:127-129` — SHA-256 on raw string |

---

## Summary

| ID | Severity | Title |
|----|----------|-------|
| F1 | High | TTL validation in commit 1 is premature (no 7-day sessions yet) |
| F2 | High | Both commits undersized; merge into one commit |
| F3 | Low | Logout endpoint routing not specified |
| F4 | Low | `/token/revoke` gives misleading error for cookie users |
| F5 | Low | Import cleanup needs precision |

**Recommended action:** Merge the two commits into one. Move TTL
validation alongside the session TTL extension code. Specify logout
routing.
