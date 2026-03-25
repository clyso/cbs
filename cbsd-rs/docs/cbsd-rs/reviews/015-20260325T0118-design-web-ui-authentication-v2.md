# Design Review: Web UI Authentication — BFF Session Cookies (v2)

**Design:** `015-20260325T0040-web-ui-authentication.md`
**Reviewer:** Claude (automated)
**Date:** 2026-03-25

## Verdict: Ready for Planning

All v1 findings have been addressed. The remaining findings are
minor and can be resolved during planning or implementation.

---

## v1 Finding Resolution

| v1 ID | Severity | Title | Status |
|-------|----------|-------|--------|
| F1 | High | CSRF analysis blind spot: callback GET | **Resolved.** Lines 156–172 now acknowledge the state-changing GET and correctly explain protection via OAuth state nonce. |
| F2 | High | Raw PASETO tokens in plaintext in session store | **Resolved.** Lines 255–290 clearly state "plaintext (serialized JSON)", document the threat model change with a before/after table, and provide explicit rationale for acceptance. |
| F3 | Medium | Cookie attributes rely on implicit defaults | **Resolved.** Lines 210–225 show explicit builder calls for all five attributes (`with_name`, `with_http_only`, `with_same_site`, `with_path`, `with_secure`). |
| F4 | Medium | Token TTL shorter than session TTL | **Resolved.** Lines 668–673 add a startup constraint: `max_token_ttl_seconds` must be ≥ 604800 (7 days), enforced via config validation. |
| F5 | Low | Cookie name default misidentified | **Resolved.** Line 148 now says `tower.sid`. |

---

## New Findings

### F1 (Low) — `is_user_active()` referenced but doesn't exist as a function

Line 330 says:

> the existing `is_token_revoked()` and `is_user_active()` checks

`is_token_revoked()` is a real function (`db/tokens.rs:36`). However,
`is_user_active()` does not exist — the user active check is an
inline `if !user.active` guard in `load_authed_user()`
(`extractors.rs:148`). Minor naming inaccuracy; the logic described
is correct.

**Recommendation:** Change to "`is_token_revoked()` and the
`user.active` check" or leave as-is (the intent is clear).

---

### F2 (Low) — Token expiry within long-lived active sessions

The design correctly documents that PASETO expiry within a live
session produces 401 (lines 325–327) and that the UI handles 401 by
redirecting to login (lines 544–545). With the default 180-day
`max_token_ttl_seconds`, a daily-active user hits this after ~6
months — their session is still alive (idle timer kept resetting)
but the embedded token expires, triggering re-auth.

The startup constraint (≥ 7 days) prevents the confusing "immediate
401 on fresh session" scenario. The 180-day case is acceptable UX
(redirect to login → OAuth dance → new token → back to dashboard).

No action needed — flagging for awareness only.

---

### F3 (Info) — `max_token_ttl_seconds = 0` does not mean unlimited

The `token_create()` function (`auth/paseto.rs:54`) computes
`expires_at = now + max_ttl_secs`, so `max_ttl_secs = 0` creates a
token that expires immediately (not an infinite-TTL token). The
`CbsdTokenPayloadV1.expires` field supports `None` for infinite
TTL, but `token_create()` always sets `Some(...)`.

This means the startup constraint "≥ 604800" is clean — there's no
special "0 = unlimited" carve-out needed. Noting for the planner's
awareness.

---

### F4 (Info) — Deferred / future work items

Carried forward from v1. All items are explicitly deferred with
sound reasoning:

| Item | Status | Notes |
|------|--------|-------|
| PKCE | Out of scope | Correct — confidential client with `client_secret` |
| Refresh tokens | Out of scope | Acceptable; session expiry triggers re-auth |
| Multi-tab sharing | Out of scope | Works automatically via cookie |
| SSE auth via cookie | Out of scope | Works automatically — `EventSource` sends cookies; SSE endpoint uses `AuthUser` which gains cookie fallback |
| WebSocket auth | Not in scope | Workers use API key auth (not `AuthUser`); no browser WebSocket needed |
| Vue.js SPA | Server-side only | UI changes listed but implementation is a separate concern |

---

## Verified Claims (re-verified against codebase)

| Claim | Verified | Source |
|-------|----------|--------|
| tower-sessions default cookie name is `tower.sid` | Yes | tower-sessions 0.14.0 (Cargo.lock:3253) |
| Session layer uses `.with_signed()` (signs cookie, not store data) | Yes | `main.rs:157` |
| `token_create()` always sets `expires: Some(...)` | Yes | `auth/paseto.rs:54-57` |
| `token_hash()` computes SHA-256 without decoding | Yes | `auth/paseto.rs:127-129` |
| `load_authed_user()` checks `user.active` inline | Yes | `extractors.rs:148-153` |
| `is_token_revoked()` treats unknown tokens as revoked | Yes | `db/tokens.rs:36-48` |
| `max_token_ttl_seconds` default is 15,552,000 (180 days) | Yes | `config.rs:226-229` |
| Config validation exists at `ServerConfig::validate()` | Yes | `config.rs:233-256` |
| `session.cycle_id()` called in callback (line 315) before response | Yes | `routes/auth.rs:315-318` |
| All five cookie attributes have corresponding builder methods in tower-sessions 0.14 | Yes (3 confirmed in use, 2 standard API) | `main.rs:156-161`, tower-sessions 0.14 API |

## Summary

| ID | Severity | Title |
|----|----------|-------|
| F1 | Low | `is_user_active()` function name doesn't match code |
| F2 | Low | Token expiry within long-lived active sessions (by design) |
| F3 | Info | `max_token_ttl_seconds = 0` semantics (planner awareness) |
| F4 | Info | Deferred / future work items (acknowledged) |

No blocking findings. The design is ready for planning.
