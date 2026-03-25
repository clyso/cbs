# Web UI Authentication — BFF Session Cookies

## Overview

This design adds web UI authentication support to cbsd-rs.
The UI is a Vue.js SPA served by a separate container behind
nginx. cbsd-rs continues to serve only `/api/*`. nginx
proxies:

```
/       → UI container (static SPA)
/api/*  → cbsd-rs container
```

cbsd-rs acts as the **Backend-for-Frontend (BFF)** for the web
flow: it handles the OAuth dance, stores tokens server-side,
and issues HttpOnly session cookies. The SPA never sees or
stores a PASETO token — it authenticates implicitly via the
cookie on every `/api/*` fetch.

The CLI flow is unchanged except that the "paste this token"
HTML page is replaced by a redirect to the UI, which renders
the token in a copyable box.

### Why BFF / why not tokens in the browser

The IETF's OAuth 2.0 Security BCP (RFC 9700, January 2025)
and the browser-based apps draft (draft-ietf-oauth-browser-
based-apps-26) recommend the BFF pattern for SPAs that handle
sensitive data. The core principle is **no tokens in the
browser**: tokens stay server-side in HttpOnly cookies that
JavaScript cannot read. This eliminates the localStorage/XSS
token-theft vector entirely.

Our architecture naturally fits BFF: cbsd-rs already manages
OAuth, token creation, and session state. The only addition is
storing the PASETO token in the session and reading it back on
subsequent requests.

### References

- RFC 9700 — OAuth 2.0 Security Best Current Practice
- draft-ietf-oauth-browser-based-apps-26 — OAuth 2.0 for
  Browser-Based Applications (BFF section)
- Design 003 — cbsd-rs auth & permissions (existing)
- Design 009 — dev OAuth bypass (existing)

---

## Authentication Flows

### Web UI flow (`?client=web`)

```
 Browser                 nginx            cbsd-rs           Google
    │                      │                  │                │
    │  GET /               │                  │                │
    │─────────────────────>│                  │                │
    │  (SPA loads)         │                  │                │
    │                      │                  │                │
    │  fetch /api/auth/whoami                 │                │
    │─────────────────────>│─────────────────>│                │
    │  401 (no cookie)     │<─────────────────│                │
    │<─────────────────────│                  │                │
    │                      │                  │                │
    │  redirect → /api/auth/login?client=web  │                │
    │─────────────────────>│─────────────────>│                │
    │                      │                  │──(store nonce  │
    │                      │                  │   in session)  │
    │  302 → Google        │                  │                │
    │<─────────────────────│<─────────────────│                │
    │──────────────────────────────────────────────────────────>│
    │  (user authenticates)                                    │
    │<──────────────────────────────────────────────────────────│
    │  302 → /api/auth/callback?code=...&state=...             │
    │─────────────────────>│─────────────────>│                │
    │                      │                  │──(validate     │
    │                      │                  │   nonce,       │
    │                      │                  │   exchange     │
    │                      │                  │   code,        │
    │                      │                  │   create user, │
    │                      │                  │   create       │
    │                      │                  │   PASETO,      │
    │                      │                  │   store token  │
    │                      │                  │   in session,  │
    │                      │                  │   extend TTL   │
    │                      │                  │   to 1 week)   │
    │  302 → /             │                  │                │
    │  Set-Cookie:         │                  │                │
    │  cbsd_session=...    │                  │                │
    │  (HttpOnly,Secure,   │<─────────────────│                │
    │   SameSite=Lax,      │                  │                │
    │   Path=/)            │                  │                │
    │<─────────────────────│                  │                │
    │                      │                  │                │
    │  fetch /api/auth/whoami (cookie sent)   │                │
    │─────────────────────>│─────────────────>│                │
    │  200 {email, caps}   │<─────────────────│                │
    │<─────────────────────│                  │                │
```

After the OAuth callback, the browser has a session cookie.
Every `fetch('/api/...')` call from the SPA automatically
includes the cookie. The SPA never touches the token.

### CLI flow (`?client=cli`)

The callback redirects to the UI with the base64-encoded token
in the URL fragment:

```
302 → /#cli-token=<base64>
```

The UI detects the `#cli-token=` fragment and renders a page
with a text box containing the token and a "Copy" button. The
user copies the token and pastes it into `cbc login`.

This replaces the current server-rendered HTML page. The UX is
now consistent: CLI users who go through the web browser see
the same UI as web users, just with a token copy prompt
instead of a dashboard.

No session cookie is set for this flow — the redirect carries
the token in the fragment (client-side only, never sent to the
server).

### Dev mode (mock OAuth)

The dev mode bypass (Design 009) works identically for both
flows. The `?client=` parameter determines the response
format, not the OAuth provider. The dev redirect to
`/api/auth/callback?state=...&dev_email=...` carries the
client type through the session, same as production.

---

## Session Cookie Details

### Cookie attributes

| Attribute | Value | Rationale |
|-----------|-------|-----------|
| `HttpOnly` | `true` | JavaScript cannot read the cookie — eliminates XSS token theft |
| `Secure` | `true` (prod), `false` (dev) | Only sent over HTTPS in production; dev uses plain HTTP |
| `SameSite` | `Lax` | Blocks cross-origin POST/PUT/DELETE (CSRF protection); allows top-level navigations (login redirects) |
| `Path` | `/` | Cookie sent on all paths — required because the SPA loads from `/` and API calls go to `/api/*` |
| `Name` | `cbsd_session` | Explicit name (tower-sessions default is `tower.sid`, which is too generic) |

### Why `SameSite=Lax` is sufficient for CSRF

`SameSite=Lax` means the cookie is sent on same-site requests
and on cross-site top-level GET navigations, but NOT on
cross-site POST, PUT, DELETE, or subresource requests.

**Mutating API endpoints** (POST, PUT, DELETE) are protected
by `SameSite=Lax` directly: the browser will not send the
cookie on cross-site subresource requests using these methods.

**The OAuth callback** (`GET /api/auth/callback`) is a
state-changing GET — it creates user records, issues tokens,
and stores them in the session. `SameSite=Lax` does NOT block
this (it allows cross-site top-level GETs). However, the
callback is protected by the **OAuth state nonce**: the
handler validates `?state=` against a nonce stored in the
session during `/login`. An attacker cannot forge a matching
nonce because they don't control the victim's session data.
A cross-site request to `/api/auth/callback?code=X&state=Y`
will fail nonce validation regardless of whether the cookie
is sent.

No additional CSRF token mechanism is needed.

### Why `Path=/` and not `Path=/api`

The session cookie must be set with `Path=/` because:

1. The OAuth callback URL is `/api/auth/callback` — the cookie
   is set here by cbsd-rs.
2. The SPA's `fetch('/api/...')` calls need the cookie.
3. If `Path=/api`, the cookie would work for API calls but
   the initial redirect to `/` after login would not carry it
   for the SPA's first `whoami` check (the navigation is to
   `/`, not `/api`).

Setting `Path=/` is safe because the cookie is HttpOnly (the
UI's JavaScript on `/` cannot read it) and SameSite=Lax (no
cross-origin leakage).

---

## Session Lifetime

### Web sessions: 1-week idle timeout

Web sessions expire after **7 days of inactivity**. Every
authenticated API request from the SPA resets the idle timer.
A user who logs in on Monday and uses the UI at least once
during the work week stays logged in. A user who doesn't touch
the UI for a full week must re-authenticate.

### OAuth flow sessions: 10-minute timeout (unchanged)

The OAuth state nonce sessions keep their current 10-minute
idle timeout. These are ephemeral: they exist only between the
`/login` redirect and the `/callback` response.

### Implementation: session layer configuration

All cookie attributes must be set **explicitly** in the
`SessionManagerLayer` builder — never rely on defaults for
security-relevant properties:

```rust
let session_layer = SessionManagerLayer::new(session_store)
    .with_signed(session_key)
    .with_name("cbsd_session")
    .with_http_only(true)
    .with_same_site(SameSite::Lax)
    .with_path("/")
    .with_secure(!config.dev.enabled)
    .with_expiry(Expiry::OnInactivity(
        time::Duration::minutes(10),
    ));
```

### Implementation: per-session expiry

`tower-sessions` supports per-session expiry via
`session.set_expiry()`. The session layer's default TTL stays
at **10 minutes** (for OAuth flow sessions). After a
successful web login, the callback handler extends the
specific session:

```rust
// After storing the PASETO token in the session:
session.set_expiry(Expiry::OnInactivity(
    time::Duration::days(7),
));
```

This means:


- OAuth flow sessions that are never completed expire after
  10 minutes (current behavior).
- Web sessions that are completed expire after 7 days of
  inactivity.
- The background deletion task cleans up both.

---

## Token Lifecycle in Web Sessions

### Storage

The PASETO token is stored in the tower-sessions session data
under the key `"paseto_token"`. The session data lives in the
SQLite `tower_sessions` table as **plaintext** (serialized
JSON). The `.with_signed()` session config signs the **cookie
value** (HMAC on the session ID), preventing cookie tampering
— but it does not encrypt the data at rest in SQLite.

The token is never sent to the browser. The browser only holds
the session ID cookie.

### Threat model change: raw tokens in session store

This design introduces a new asset in the SQLite database:

| Table | Before | After |
|-------|--------|-------|
| `tokens` | SHA-256 hashes only | Unchanged |
| `tower_sessions` | OAuth nonces (ephemeral) | Raw PASETO tokens |

An attacker with read access to the SQLite file can extract
active PASETO tokens from the `tower_sessions` table and use
them as Bearer tokens.

**Why this is acceptable:**

- The DB file is local to the server container — read access
  requires a separate vulnerability (filesystem escape, backup
  leak, or host compromise).
- The PASETO symmetric encryption key (`token_secret_key`) is
  also on the same host. If both the DB and the key are
  compromised, the attacker can forge arbitrary tokens anyway
  — the session store tokens add no incremental risk.
- Web session tokens have a shorter effective lifetime (1 week
  idle timeout) than CLI tokens (6 months default).
- The expired session cleanup task runs every 60 seconds,
  limiting the window for stale token extraction.

### Validation

On each request, the `AuthUser` extractor checks two sources
in order:

1. `Authorization: Bearer <token>` header — if present,
   validate as PASETO or API key (existing path, unchanged).
2. If no header: load the session, read `"paseto_token"`, and
   validate the PASETO token through the same code path
   (decode, check expiry, check revocation in DB, check
   `users.active`).

If neither source provides a valid identity, return 401.

This means token revocation, user deactivation, and TTL
enforcement work identically for both cookie-authenticated web
users and header-authenticated CLI/API users. There is no
separate "session validity" concept — the session is just a
container for the PASETO token, and the token is the source of
truth.

### Why store the PASETO token, not just a user ID

Storing the full PASETO token in the session (rather than just
the user email) preserves all existing security properties:

- **Token revocation works.** An admin who revokes a user's
  tokens via `POST /api/auth/tokens/revoke-all` immediately
  invalidates their web session — the next request extracts
  the PASETO from the session, checks the revocation table,
  and returns 401.
- **User deactivation works.** The `AuthUser` extractor checks
  `users.active` on every request regardless of auth source.
- **Token TTL works.** The PASETO's `expires` claim is checked
  on every request. A token that expires while the session is
  still alive produces a 401.
- **No new code for invalidation.** The existing
  `is_token_revoked()` and `user.active` checks apply to
  both paths without modification.

If we stored only the user email, we'd need a separate
mechanism to invalidate web sessions (a session revocation
table, or forcibly deleting session rows) — duplicating logic
that PASETO revocation already provides.

---

## AuthUser Extractor Changes

### Current extractor logic

```
1. Read Authorization: Bearer <token>
2. If prefix is "cbsk_" → API key path (argon2 + LRU cache)
3. Else → PASETO path (decode, check revocation, check active)
4. Load effective capabilities
5. Return AuthUser { email, name, caps }
```

### New extractor logic

```
1. Read Authorization: Bearer <token>
   → if present, proceed to step 2 (existing path)
   → if absent, go to step 1b

1b. Load session (tower-sessions Session extractor)
    → read "paseto_token" from session data
    → if present, use as the raw token and proceed to step 3
    → if absent, return 401

2. If prefix is "cbsk_" → API key path (unchanged)
3. PASETO validation (unchanged — same for header and session)
4. Load effective capabilities (unchanged)
5. Return AuthUser { email, name, caps }
```

The key change is step 1b: a fallback from the Authorization
header to the session cookie. The rest of the pipeline is
identical.

### Session extractor in axum

`tower-sessions` provides a `Session` extractor that reads the
cookie automatically. The `AuthUser` extractor needs access to
it. Two approaches:

**Option A — extract Session inside AuthUser:**
`FromRequestParts` implementations can extract other
extractors. `AuthUser`'s `from_request_parts` can call
`Session::from_request_parts(parts, state)` internally when
no Authorization header is found.

**Option B — wrap in a middleware:**
A middleware layer checks the cookie and injects a synthetic
`Authorization` header before the request reaches the handler.
This is transparent to handlers but feels like a hack.

**Recommendation: Option A.** It keeps the logic in one place
(the `AuthUser` extractor) and doesn't mutate request headers.

---

## New Endpoint: `POST /api/auth/logout`

Clears the web session and revokes the underlying token.

```
POST /api/auth/logout
Authorization: (none — uses session cookie)

Response: 200 {"detail": "logged out"}
Set-Cookie: cbsd_session=; Max-Age=0; Path=/; HttpOnly; ...
```

### Handler logic

1. Load session.
2. Read `"paseto_token"` from session.
3. If token exists, compute its SHA-256 hash and call
   `db::tokens::revoke_token()` (same as `POST /token/revoke`
   but sourced from session instead of header).
4. Delete the session (`session.flush()`).
5. Return 200.

This endpoint does NOT require the `AuthUser` extractor (the
session may contain an expired or revoked token that fails
validation — the user should still be able to log out). It
reads the session directly.

---

## Callback Handler Changes

### Current behavior (Design 003)

| `client` | Response |
|-----------|----------|
| `cli` | HTML page displaying base64 token (or JS redirect to localhost with `cli_port`) |
| `web` | Redirect to `/#token=<base64>` |

### New behavior

| `client` | Response |
|-----------|----------|
| `cli` | Redirect to `/#cli-token=<base64>` |
| `web` | Store token in session, redirect to `/` |

Changes:

1. **`client=web`:** Instead of redirecting with
   `/#token=<base64>`, store the PASETO token in the session
   under `"paseto_token"`, extend the session TTL to 7 days,
   and redirect to `/`. The browser receives a `Set-Cookie`
   with the session ID.

2. **`client=cli`:** Instead of rendering a server-side HTML
   page (or the localhost redirect with `cli_port`), redirect
   to `/#cli-token=<base64>`. The UI renders a styled token
   copy page with a "Copy to clipboard" button.

### Removed: `cli_port` localhost redirect

The `cli_port` query parameter and the localhost JS redirect
flow are removed entirely. The CLI authenticates by opening
the browser to `/api/auth/login?client=cli`, the user copies
the token from the UI, and pastes it into `cbc login`. This
is simpler and avoids the need for the CLI to spawn a
temporary HTTP server.

**Code to remove** (currently implemented in
`cbsd-server/src/routes/auth.rs`):

- `LoginQuery.cli_port: Option<u16>` field (line 94)
- Session insert of `cli_port` in `login` handler (line 166)
- Session read of `cli_port` in `callback` handler (line 234)
- The `if let Some(port) = cli_port` branch in callback that
  renders the JS redirect HTML page (line 325)
- The server-rendered HTML page with the CSP header for the
  CLI-without-port case (lines 337-353)

### Token creation

For both flows, a PASETO token is still created and stored in
the `tokens` DB table. The difference is only in delivery:

- CLI flow: token is delivered to the browser via URL fragment
  for the user to copy.
- Web flow: token is stored server-side in the session. The
  client never sees it.

---

## UI Responsibilities

The Vue.js SPA needs to handle these auth-related concerns:

### 1. Login check on load

```javascript
const resp = await fetch('/api/auth/whoami')
if (resp.status === 401) {
  // Not logged in — show login page/button
} else {
  // Logged in — load dashboard
  const user = await resp.json()
}
```

No token management, no `Authorization` header construction.
The browser sends the session cookie automatically.

### 2. Login initiation

```javascript
window.location.href = '/api/auth/login?client=web'
```

The server handles the full OAuth round-trip and redirects
back to `/` with a cookie set.

### 3. CLI token display

On load, check for `#cli-token=` in the URL hash:

```javascript
const hash = window.location.hash
if (hash.startsWith('#cli-token=')) {
  const token = hash.substring('#cli-token='.length)
  // Render token copy UI
  // Clear the hash from the URL bar
}
```

This page shows a text box with the base64 token and a
"Copy to clipboard" button. The UX matches the web UI's
visual style.

### 4. Logout

```javascript
await fetch('/api/auth/logout', { method: 'POST' })
window.location.href = '/'
```

### 5. API calls

```javascript
// No Authorization header needed — cookie is automatic
const builds = await fetch('/api/builds')
```

If any API call returns 401, the UI should redirect to the
login flow (the session has expired or been revoked).

---

## nginx Configuration

Minimal nginx config for the proxy setup:

```nginx
server {
    listen 443 ssl;
    server_name cbs.example.com;

    # UI (static SPA)
    location / {
        proxy_pass http://ui:3000;
    }

    # API (cbsd-rs)
    location /api/ {
        proxy_pass http://cbsd-rs:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

The cookie works because both `/` and `/api/` are on the same
origin (`cbs.example.com`). The `Set-Cookie` from
`/api/auth/callback` with `Path=/` is sent on all subsequent
requests to the same origin, regardless of path.

---

## Security Considerations

### Token-in-fragment for CLI flow

The `/#cli-token=<base64>` fragment is never sent to the
server (URL fragments are client-side only). The token is
visible in the browser's address bar and potentially in
browser history. This is acceptable for the CLI flow because:

- The user explicitly initiated this flow to obtain a token
  for manual copy-paste.
- The token is a long-lived PASETO (same as what `cbc login`
  stores on disk).
- The fragment is cleared by the UI after rendering.

### Session fixation

The callback handler already calls `session.cycle_id()` after
OAuth validation (Design 003). This prevents session fixation
by regenerating the session ID before storing the token.

### Cookie theft via subdomain

If the deployment uses a shared parent domain (e.g.,
`cbs.example.com` and `other.example.com`), a compromised
sibling subdomain could read cookies scoped to `.example.com`.
The session cookie is set on the exact host
(`cbs.example.com`), not the parent domain, preventing this.
nginx must not set a `Domain=` attribute on the cookie (and
it won't — cbsd-rs sets the cookie, nginx just proxies it).

### Token revocation propagation

When an admin revokes a user's tokens (via
`POST /api/auth/tokens/revoke-all` or user deactivation),
the web session becomes invalid on the **next API request**
(the PASETO validation fails). There is no mechanism to push
an invalidation to the browser. The session cookie remains in
the browser until the next request returns 401, at which point
the UI redirects to login.

This is the same latency as the current Bearer-token model and
is acceptable for our use case.

---

## Changes Summary

### Server changes (cbsd-rs)

| Area | Change |
|------|--------|
| `routes/auth.rs` callback | Web: store token in session, redirect to `/`. CLI: redirect to `/#cli-token=<base64>` |
| `routes/auth.rs` | New `POST /api/auth/logout` endpoint |
| `routes/auth.rs` login | Remove `cli_port` parameter and localhost redirect logic |
| `auth/extractors.rs` | `AuthUser`: fall back to session cookie when no Authorization header |
| `main.rs` session config | Explicit cookie attributes (see below), keep 10-minute default TTL |
| Removed | `/#token=<base64>` redirect (web flow), server-rendered HTML token page (CLI flow), `cli_port` localhost redirect |

### UI changes (new, Vue.js SPA)

| Area | Description |
|------|-------------|
| Auth check | `fetch('/api/auth/whoami')` on load |
| Login | Redirect to `/api/auth/login?client=web` |
| CLI token page | Detect `#cli-token=` hash, render copy UI |
| Logout | `POST /api/auth/logout` |
| API calls | Plain `fetch()` — cookie sent automatically |

### nginx

Standard reverse proxy config. No auth-specific logic.

---

## Out of Scope

- **PKCE (Proof Key for Code Exchange):** Not needed because
  cbsd-rs is a confidential client (has a `client_secret`).
  PKCE is required for public clients (SPAs doing OAuth
  directly). Since cbsd-rs handles the OAuth exchange
  server-side, the authorization code is never exposed to the
  browser.
- **Refresh tokens:** PASETO tokens are long-lived (up to
  `max_token_ttl_seconds`). The session idle timeout (1 week)
  is shorter than the token TTL. When the session expires, the
  user re-authenticates and gets a new token. No refresh
  mechanism needed. **Constraint:** `max_token_ttl_seconds`
  must be ≥ 604800 (7 days) — if the operator sets a shorter
  TTL, the PASETO embedded in the session expires before the
  session idle timeout, producing confusing 401s on an
  apparently live session. The server should validate this at
  startup and refuse to start if the constraint is violated.
- **Multi-tab session sharing:** Sessions are cookie-based, so
  all tabs on the same origin share the same session
  automatically. No special handling needed.
- **SSE/WebSocket authentication:** The existing SSE log
  streaming endpoints will receive the cookie like any other
  request. No changes needed for `EventSource` — browsers send
  cookies on SSE connections by default.
