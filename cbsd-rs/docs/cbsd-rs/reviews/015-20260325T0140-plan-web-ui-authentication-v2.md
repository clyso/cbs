# Plan Review: Web UI Authentication (v2)

**Plan:** `015-20260325T0129-web-ui-authentication.md`
**Design:** `015-20260325T0040-web-ui-authentication.md`
**Reviewer:** Claude (automated)
**Date:** 2026-03-25

## Verdict: Ready for Implementation

All v1 findings resolved. Single commit at ~180 authored lines
passes all five smell tests. No new issues found.

---

## v1 Finding Resolution

| v1 ID | Severity | Title | Status |
|-------|----------|-------|--------|
| F1 | High | TTL validation premature in commit 1 | **Resolved.** Merged into single commit — TTL validation and the 7-day session feature land together. |
| F2 | High | Both commits undersized; should be one | **Resolved.** Now a single commit at ~180 authored lines. |
| F3 | Low | Logout endpoint routing not specified | **Resolved.** Lines 143–153 explicitly place logout in `oauth_routes` (rate-limited via tower-governor). |
| F4 | Low | `/token/revoke` misleading error for cookie users | **Resolved.** Lines 190–207 add a guard returning 400 "use /api/auth/logout for web sessions" when no Authorization header is present. |
| F5 | Low | Import cleanup imprecise | **Resolved.** Lines 209–216 specify exactly which items to remove from each combined import line; `HeaderMap` stays. |

---

## Commit Boundary Validation

### Smell test: single commit

| Test | Pass | Evidence |
|------|------|----------|
| One-sentence purpose | Yes | "Add BFF session cookie authentication for web UI users" |
| Previous commit compiles | Yes | Modifies existing working auth subsystem |
| Revertable | Yes | Reverting removes entire BFF feature cleanly |
| Testable | Yes | 9 test scenarios listed (lines 218–231) |
| No dead code | Yes | Every piece has immediate callers: extractor reads what callback stores, logout clears what callback created, TTL validation guards the session TTL the callback sets, guard protects existing endpoint |

### Sizing

~180 authored lines. Below the 400–800 target but above the 200-line
floor. Acceptable for a focused, cohesive feature — the change is
naturally this size and shouldn't be padded.

---

## Verified Claims

| Claim | Status | Evidence |
|-------|--------|----------|
| `session.insert().await` is async | Correct | `routes/auth.rs:152–158` |
| `session.get().await` is async | Correct | `routes/auth.rs:208` |
| `session.set_expiry()` is sync (no `.await`) | Consistent | In-memory metadata operation; no store interaction |
| `session.flush().await` is async | Consistent | Store-interaction operation (deletes session row) |
| Callback stores `raw_token` (not `token_b64`) | Correct | `token_decode()` expects raw `v4.local.xxx` string, not base64 |
| Session extractor creates empty session when no cookie | Correct | tower-sessions 0.14 behavior; empty sessions are not persisted |
| Bearer-token requests skip session extraction | Correct | Step 1 succeeds → never reaches step 1b |
| `revoke_token` handler reads `Authorization` header directly | Correct | `routes/auth.rs:400–404` — guard needed for cookie users |
| `HeaderMap` still used after HTML removal | Correct | `routes/auth.rs:397` — `revoke_token` handler |
| `oauth_routes` has governor rate limiting | Correct | `routes/auth.rs:70` — `governor_layer` applied |

---

## Notes for Implementer

1. **Magic number `7`** appears in two places: the config validation
   constant (`7 * 24 * 3600`) and the session expiry
   (`time::Duration::days(7)`). Consider extracting a shared constant
   to prevent drift.

2. **`SameSite` import path**: The plan correctly notes to verify
   `tower_sessions::cookie::SameSite` against the 0.14 API at
   implementation time.

3. **Empty sessions on 401 responses**: When no Authorization header
   and no session cookie exist, the extractor creates an empty
   `Session`, finds no `"paseto_token"`, and returns 401. tower-sessions
   does NOT persist empty unmodified sessions, so this does not leak
   `Set-Cookie` headers on unauthenticated 401 responses.

---

## Summary

No findings. Ready for implementation.
