# Design Review: Web UI Authentication — BFF Session Cookies (v1)

**Design:** `015-20260325T0040-web-ui-authentication.md`
**Reviewer:** Claude (automated)
**Date:** 2026-03-25

## Verdict: Needs Revision

The design is architecturally sound — the BFF pattern, the
"token-in-session" approach, and the dual-flow (web cookie / CLI
fragment) are all well-reasoned. However, there are several factual
inaccuracies, a gap in the CSRF analysis, and a threat-model change
that the document doesn't acknowledge.

---

## Findings

### F1 (High) — CSRF analysis has a blind spot: `GET /api/auth/callback` is state-changing

The design states (line 155–156):

> Since cbsd-rs has no state-changing GET endpoints (all mutations use
> POST/PUT/DELETE), a cross-origin attacker cannot trigger mutations
> with the victim's cookie.

This is **incorrect**. `GET /api/auth/callback` creates a user record,
creates a PASETO token, inserts the token into the `tokens` table, and
(after this design) stores it in the session. It is a GET endpoint that
mutates server state.

**Why SameSite=Lax doesn't matter here:** Lax allows cookies on
cross-site top-level GET navigations. An attacker could craft a link
to `/api/auth/callback?code=X&state=Y`. The cookie would be sent.

**Why it's still safe:** The callback validates the `state` parameter
against a nonce stored in the session during `/login`. An attacker
cannot forge a matching nonce because they don't control the victim's
session data. CSRF protection for the callback comes from the **OAuth
state parameter**, not from SameSite.

**Recommendation:** Correct the CSRF section to acknowledge this
endpoint and explain why it's protected by the OAuth state nonce rather
than by the absence of state-changing GETs.

---

### F2 (High) — Raw PASETO tokens stored in plaintext in SQLite session store

The design says (line 225–227):

> The session data lives in the SQLite `tower_sessions` table,
> encrypted/signed by the session layer's key

This is **misleading**. The `.with_signed(session_key)` configuration
signs the **session ID cookie** (HMAC), preventing tampering of the
cookie value. It does NOT encrypt the session data in the SQLite
store. The `tower_sessions` table holds session data as **plain
serialized data** (typically JSON or MessagePack).

This means the raw PASETO token string is stored in plaintext in
SQLite. This changes the threat model compared to today:

| Asset | Current | After this design |
|-------|---------|-------------------|
| `tokens` table | Stores **hashes** — DB access alone cannot impersonate | Unchanged |
| `tower_sessions` table | No tokens stored (OAuth nonces only) | Stores **raw PASETO tokens** — DB read access allows impersonation |

An attacker with read access to the SQLite file can extract active
PASETO tokens from the `tower_sessions` table and use them as Bearer
tokens.

**Mitigating factors:**


- The DB file is local to the server — remote access requires a
  separate vulnerability.
- The PASETO signing key is also on disk; if both are compromised,
  arbitrary token creation is possible anyway.
- Session cleanup runs every 60 seconds for expired sessions.


**Recommendation:** Either:

1. Accept the risk and document it explicitly as a known threat-model
   change (most pragmatic).
2. Use `tower-sessions`' `.with_private(key)` mode — but note this
   only encrypts the cookie, not the store data, so it doesn't solve
   the problem either.
3. Implement a thin encrypting wrapper around `SqliteStore` that
   encrypts session data at rest.

Option 1 is likely sufficient for the deployment model. But the design
must not claim the session data is "encrypted/signed" — it isn't.

---

### F3 (Medium) — Cookie attributes rely on implicit defaults

The design specifies `HttpOnly=true`, `SameSite=Lax`, and `Path=/` in
the cookie attributes table (lines 142–148), but the Changes Summary
(line 577) only mentions setting the cookie name to `cbsd_session`.
The current session layer configuration does not explicitly set:

- `SameSite` (tower-sessions may default to `Lax`, but relying on
  implicit defaults for security properties is fragile)
- `Path` (defaults to `/` in most implementations, but should be
  explicit)
- `HttpOnly` (tower-sessions defaults to `true` for signed cookies,
  but again — explicit is better for security)

**Recommendation:** The design should specify that all four cookie
attributes (`HttpOnly`, `Secure`, `SameSite`, `Path`) are set
explicitly in the `SessionManagerLayer` builder chain. Example:

```rust
.with_name("cbsd_session")
.with_http_only(true)
.with_same_site(SameSite::Lax)
.with_path("/")
```

---

### F4 (Medium) — Token TTL shorter than session TTL is unaddressed

The Out of Scope section (line 604–608) states:

> PASETO tokens are long-lived (up to `max_token_ttl_seconds`). The
> session idle timeout (1 week) is shorter than the token TTL.

The default `max_token_ttl_seconds` is 180 days (6 months), so this
holds for the default config. However, `max_token_ttl_seconds` is
operator-configurable (`config.rs:78`). If an operator sets it to,
say, 1 day (86400 seconds), the PASETO token embedded in the session
expires well before the 7-day session idle timeout.

In this scenario: the session cookie is still valid, the session store
still has the token, but the token's `expires` claim fails validation.
The user gets 401 on every request despite having a valid session. The
only recovery is to re-authenticate — but the user experience is
confusing because the session appears alive.


**Recommendation:** Either:

1. Document the constraint: `max_token_ttl_seconds` MUST be ≥ session
   idle timeout (7 days = 604800 seconds). Add a config validation
   check.
2. When creating web-flow tokens, set TTL to
   `max(max_token_ttl_seconds, 7 days)` — but this undermines the
   operator's intention of short-lived tokens.
3. Detect "token expired but session alive" in the AuthUser extractor
   and return a distinct error (e.g., 401 with a `token_expired`
   code) so the UI can auto-redirect to re-auth.

Option 1 is simplest and most transparent.

---

### F5 (Low) — Cookie name default is misidentified

The design states (line 148):

> `tower-sessions` default is `id`, which is too generic

In tower-sessions 0.14, the default cookie name is `tower.sid`, not
`id`. The recommendation to rename to `cbsd_session` is still correct.

**Recommendation:** Fix the parenthetical to say `tower.sid`.

---

### F6 (Info) — Deferred / future work items

The following are explicitly deferred. Flagging for tracking:

| Item | Status | Notes |
|------|--------|-------|
| PKCE | Out of scope | Correct — cbsd-rs is a confidential client with `client_secret` |
| Refresh tokens | Out of scope | Acceptable given long-lived PASETO + session idle timeout |
| Multi-tab session sharing | Out of scope | Works automatically via cookie — no action needed |
| SSE auth via cookie | Out of scope | Works automatically — `EventSource` sends cookies by default. Verified: SSE endpoint uses `AuthUser` extractor, which will gain cookie fallback |
| WebSocket auth | Not mentioned | Workers use API key auth (manual, not `AuthUser`). No browser WebSocket needed. Correctly not in scope |
| Vue.js SPA implementation | In scope (UI side) | Server-side only changes in cbsd-rs; UI is a separate concern |

---

## Verified Claims

The following claims were verified against the codebase:

| Claim | Verified | Source |
|-------|----------|--------|
| `LoginQuery` has `cli_port: Option<u16>` | Yes | `routes/auth.rs:94` |
| Callback handler has localhost JS redirect branch | Yes | `routes/auth.rs:325-335` |
| Callback handler has server-rendered HTML token page | Yes | `routes/auth.rs:337-352` |
| Web client redirects to `/#token=<base64>` | Yes | `routes/auth.rs:354-357` |
| `session.cycle_id()` called in callback | Yes | `routes/auth.rs:315` |
| Session uses SQLite store with 10-minute expiry | Yes | `main.rs:156-161` |
| Session key derived via HKDF from `token_secret_key` | Yes | `main.rs:145-152` |
| `AuthUser` only checks `Authorization: Bearer` | Yes | `extractors.rs:178-187` (no cookie fallback) |
| `whoami` endpoint exists, returns email/name/caps/roles | Yes | `routes/auth.rs:365-387` |
| No `logout` endpoint exists | Yes | Not found in `routes/auth.rs` router |
| Token revocation via `db::tokens::revoke_token()` | Yes | `db/tokens.rs:51-60` |
| `revoke_all_for_user()` exists | Yes | `db/tokens.rs:63-72` |
| `is_token_revoked()` treats unknown tokens as revoked | Yes | `db/tokens.rs:36-48` |
| `max_token_ttl_seconds` default is 6 months | Yes | `config.rs:226-229` |
| SSE log endpoint uses `AuthUser` | Yes | `routes/builds.rs:499` |
| WebSocket uses manual API key auth (not `AuthUser`) | Yes | `ws/handler.rs:42-55` |
| `tower-sessions` 0.14 with `signed` feature | Yes | `cbsd-server/Cargo.toml:38` |
| Background expired session cleanup every 60s | Yes | `main.rs:164-168` |
| Dev mode disables `Secure` flag | Yes | `main.rs:158` |
| Token creation stores hash in `tokens` table | Yes | `routes/auth.rs:296-312` |

## Summary

| ID | Severity | Title |
|----|----------|-------|
| F1 | High | CSRF analysis has a blind spot: callback GET is state-changing |
| F2 | High | Raw PASETO tokens stored in plaintext in session store |
| F3 | Medium | Cookie attributes rely on implicit defaults |
| F4 | Medium | Token TTL shorter than session TTL is unaddressed |
| F5 | Low | Cookie name default is misidentified |
| F6 | Info | Deferred / future work items (acknowledged) |

F1 and F2 require text revisions to the design document. F3 and F4
require design decisions (can be resolved inline). F5 is a minor
factual correction.
