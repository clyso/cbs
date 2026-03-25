# 01 — Authentication

## Overview

OAuth login flow, token storage, and user identity commands.

## Commands

### `cbc login <url>`

Initiates the Google OAuth login flow.

```
$ cbc login https://cbs.example.com

Opening browser for authentication...

  https://cbs.example.com/api/auth/login?client=cli

If the browser doesn't open, visit the URL above.
Paste the token here: █
```

**Flow:**

1. Validate the server is reachable:
   `GET /api/health`. On failure:
   `"cannot reach server at <url>"`.
2. Construct the login URL:
   `{url}/api/auth/login?client=cli`.
3. Open the URL in the default browser via
   `open::that()`. Print the URL regardless (fallback
   for headless environments).
4. Prompt the user to paste the token (the server's
   callback page displays it as a copyable string).
5. Validate the token: `GET /api/auth/whoami` with the
   pasted token. On failure: `"invalid token"`.
6. Save the config to `~/.config/cbc/config.json`:
   `{"host": "<url>", "token": "<pasted-token>"}`.
7. Print: `"logged in as <email>"`.

**Arguments:**

- `URL` (required) — server base URL.

**Out of scope (first iteration):** The server supports a
`cli_port` parameter for automatic localhost redirect
(avoids copy-paste). This requires the client to bind a
local HTTP listener. Deferred to a future enhancement.

### `cbc whoami`

Displays the authenticated user's identity and roles.

```
$ cbc whoami

  email: admin@clyso.com
   name: Admin
  roles: admin
   caps: *
```

**Endpoint:** `GET /api/auth/whoami`

**Response fields displayed:**


- `email`
- `name`
- `roles` (comma-separated)
- `effective_caps` (comma-separated, or `*`)

Requires a valid config file with token.

Note: scopes are not shown in `whoami` output. Users
with scope-restricted roles (e.g., `builds:create`
scoped to a specific channel) should use
`cbc admin users get <email>` for full role details
including scope constraints.

## Token lifecycle

- Tokens are PASETO v4 with a server-configured TTL
  (default 24 hours). The client does not manage
  expiry — on 401, the user must `cbc login` again.
- The client does not parse or validate tokens locally
  — it sends them as-is in the `Authorization` header.
- On 401 response from any command: print
  `"session expired — run 'cbc login {host}' to
  re-authenticate"` (using the stored host from config).
- No automatic token refresh.

## Config file interaction

`cbc login` is the only command that writes the config.
All other commands read it.

If the config file exists but the token is expired
(server returns 401), the client does NOT delete the
config — it preserves the `host` field so the user can
`cbc login` again without re-typing the URL.
