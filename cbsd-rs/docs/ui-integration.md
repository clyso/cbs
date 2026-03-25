# Web UI Integration Guide

This document describes how a frontend SPA integrates with
the cbsd-rs API server for authentication, session management,
and API access. It is written for frontend engineers building
the CBS web UI.

---

## Architecture

The CBS web UI is a Vue.js SPA served as static files by its
own container. An nginx reverse proxy sits in front of both
the UI and the API server:

```
Browser
  │
  ├── GET /              → nginx → UI container (static SPA)
  ├── GET /assets/*      → nginx → UI container
  └── */api/*            → nginx → cbsd-rs container
```

The SPA and the API share the same origin (same hostname and
port). This is important: the session cookie set by cbsd-rs
on `/api/auth/callback` is sent by the browser on all
subsequent requests to the same origin, including
`fetch('/api/...')` calls from the SPA.

cbsd-rs does not serve the UI. It only serves the `/api/*`
endpoints.

### nginx configuration

```nginx
server {
    listen 443 ssl;
    server_name cbs.example.com;

    # SPA (static files)
    location / {
        proxy_pass http://ui:3000;
    }

    # API server
    location /api/ {
        proxy_pass http://cbsd-rs:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For
            $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Adjust container hostnames and ports to match the deployment.
The key requirement is that both `/` and `/api/` resolve to
the same origin from the browser's perspective.

---

## Authentication Model

cbsd-rs uses the **Backend-for-Frontend (BFF)** pattern. The
SPA never handles, stores, or transmits authentication tokens.
Instead:

1. The server performs the full OAuth exchange server-side.
2. After successful authentication, the server stores the
   token in a server-side session and sends the browser an
   HttpOnly session cookie.
3. The browser automatically includes this cookie on every
   `/api/*` request.
4. The SPA authenticates implicitly — no `Authorization`
   header, no `localStorage`, no token management code.

### Session cookie properties

| Attribute | Value |
|-----------|-------|
| Name | `cbsd_session` |
| HttpOnly | `true` (JavaScript cannot read it) |
| Secure | `true` in production, `false` in dev |
| SameSite | `Lax` |
| Path | `/` |

### Session lifetime

The session expires after **7 days of inactivity**. Every API
request resets the idle timer. A user who interacts with the
UI at least once per work week stays logged in.

---

## Authentication Flows

### Web login (production — Google OAuth)

```
1. SPA navigates to:
   /api/auth/login?client=web

2. cbsd-rs redirects the browser to Google's OAuth consent
   screen. The user authenticates with their Google account.

3. Google redirects back to:
   /api/auth/callback?code=...&state=...

4. cbsd-rs validates the OAuth response, creates the user
   record (if first login), creates a server-side session,
   and redirects to:
   /
   with a Set-Cookie header for `cbsd_session`.

5. The SPA loads and calls:
   GET /api/auth/whoami
   The session cookie is sent automatically. The server
   responds with user info.
```

The SPA's only role is to initiate the flow (step 1) and
check the result (step 5). Everything in between is handled
by the server and the browser's redirect machinery.

### Web login (dev mode — mock OAuth)

In dev mode (`dev.enabled: true` in server config), Google is
bypassed entirely. The flow is identical from the SPA's
perspective:

```
1. SPA navigates to:
   /api/auth/login?client=web

2. cbsd-rs skips Google and redirects directly to its own
   callback with the seed admin email. No external network
   calls are made.

3. cbsd-rs creates the session and redirects to:
   /
   with the session cookie set.
```

The SPA does not need to know whether dev mode is active. The
`/api/auth/login?client=web` URL works identically in both
modes.

### CLI token flow

When a CLI user authenticates through the browser (e.g., for
`cbc login`), cbsd-rs redirects to the SPA with a token in
the URL fragment:

```
/api/auth/login?client=cli
  → (OAuth or mock flow)
  → 302 /#cli-token=<base64-encoded-token>
```

The SPA must detect this fragment on load and render a token
copy UI.

---

## What the SPA Must Implement

### 1. Auth check on load

On every page load (or route change), check if the user is
authenticated:

```javascript
const resp = await fetch('/api/auth/whoami')
if (resp.status === 401) {
  // Not authenticated — show login page or redirect
} else {
  const user = await resp.json()
  // user = { email, name, roles, effective_caps }
  // Store in app state for the session
}
```

No `Authorization` header is needed. The browser sends the
`cbsd_session` cookie automatically.

### 2. Login button

```javascript
function login() {
  window.location.href = '/api/auth/login?client=web'
}
```

This is a full-page navigation, not a `fetch()` call. The
browser follows the redirect chain (server → Google → server
→ SPA) and returns to `/` with the session cookie set.

### 3. CLI token display

On load, check for the `#cli-token=` fragment:

```javascript
const hash = window.location.hash
if (hash.startsWith('#cli-token=')) {
  const token = hash.substring('#cli-token='.length)

  // Render a UI with:
  // - The token in a read-only text field
  // - A "Copy to clipboard" button
  // - A message like "Paste this token into your CLI"

  // Clear the fragment from the URL bar to prevent
  // accidental sharing or bookmark leakage
  history.replaceState(null, '', '/')
}
```

This page should be styled consistently with the rest of the
UI. The token is a base64-encoded PASETO token string.

### 4. Logout

```javascript
async function logout() {
  await fetch('/api/auth/logout', { method: 'POST' })
  // Session cookie is cleared by the server's response
  window.location.href = '/'
}
```

After logout, `/api/auth/whoami` returns 401.

### 5. API calls

All API calls use plain `fetch()` with no special headers:

```javascript
const resp = await fetch('/api/builds')
const builds = await resp.json()
```

The session cookie is included automatically by the browser.

### 6. Handling 401 responses

If any API call returns 401, the session has expired or been
revoked. The SPA should redirect the user to the login flow:

```javascript
async function apiFetch(url, options = {}) {
  const resp = await fetch(url, options)
  if (resp.status === 401) {
    // Session expired or revoked
    window.location.href = '/api/auth/login?client=web'
    return
  }
  return resp
}
```

---

## API Endpoints Reference

### Authentication

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/auth/login?client=web` | None | Initiates OAuth. Redirects to Google (or mock). |
| GET | `/api/auth/login?client=cli` | None | Same, but callback redirects to `/#cli-token=`. |
| GET | `/api/auth/callback` | None | OAuth callback (handled by server, not called by SPA). |
| GET | `/api/auth/whoami` | Cookie | Returns `{ email, name, roles, effective_caps }`. |
| POST | `/api/auth/logout` | Cookie | Clears session cookie, revokes token. |

### Error responses

All API errors use the shape:

```json
{"detail": "human-readable error message"}
```

Common status codes:

| Code | Meaning |
|------|---------|
| 401 | Not authenticated (no cookie or expired session) |
| 403 | Authenticated but lacking the required capability |
| 404 | Resource not found (or hidden by ownership check) |
| 409 | Conflict (e.g., last-admin guard) |
| 429 | Rate limited (login/callback/logout) |

---

## CORS

CORS is not needed. The SPA and the API are on the same
origin (same scheme + hostname + port, proxied through nginx).
Same-origin requests are not subject to CORS restrictions.

Do not add `credentials: 'include'` to `fetch()` calls — it
is only needed for cross-origin requests. Same-origin requests
send cookies by default.

---

## Things the SPA Does NOT Need to Do

- **Store tokens.** The SPA never sees a token (except the
  CLI flow fragment, which is for copy-paste only).
- **Set `Authorization` headers.** Cookie auth is automatic.
- **Implement CSRF protection.** `SameSite=Lax` on the
  cookie handles this.
- **Handle refresh tokens.** The server manages token and
  session lifecycle. When the session expires, the user
  re-authenticates via the standard login flow.
- **Distinguish dev mode from production.** The same
  `/api/auth/login?client=web` URL works in both modes.

---

## Dev Environment Setup

For local development, the SPA dev server (e.g., `vite dev`)
and cbsd-rs run on different ports. Use a local nginx or
vite's proxy config to unify them under one origin:

### Option A: vite proxy

```javascript
// vite.config.js
export default defineConfig({
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
    },
  },
})
```

This makes `fetch('/api/...')` from the SPA dev server proxy
to cbsd-rs. The session cookie works because the browser sees
one origin (`localhost:5173`).

### Option B: local nginx

Same config as production but with `http://` and local ports.

### Dev mode auth

cbsd-rs in dev mode (`dev.enabled: true`) skips Google OAuth
entirely. Clicking "Login" redirects through the mock flow
and back to the SPA in under a second, with the seed admin
user authenticated. No Google credentials or network access
needed.
