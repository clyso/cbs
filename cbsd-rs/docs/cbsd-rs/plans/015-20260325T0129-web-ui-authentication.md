# Plan — Web UI Authentication (Design 015)

## Progress

| Item | Status |
|------|--------|
| Commit 1: BFF session cookie authentication for web UI | Done |

## Goal

Web UI users authenticate via HttpOnly session cookies (BFF
pattern). CLI users get a token via URL fragment for copy-paste.
The `cli_port` localhost redirect is removed. All cookie
attributes are set explicitly.

## Depends on

None — this is a new feature on the existing auth subsystem.

---

## Commit 1: BFF session cookie authentication for web UI

Single commit covering session config hardening, the extractor
fallback, callback rewrite, logout endpoint, TTL validation,
and `cli_port` removal. All pieces are tightly coupled: the
config sets up the cookie attributes the extractor reads, the
extractor reads the token the callback stores, the logout
endpoint clears what the callback created, and the TTL
validation guards the session idle timeout the callback sets.

### `main.rs` — explicit session cookie attributes

Replace the current session layer builder:

```rust
// Current:
let session_layer = SessionManagerLayer::new(session_store)
    .with_signed(session_key)
    .with_secure(!config.dev.enabled)
    .with_expiry(Expiry::OnInactivity(
        time::Duration::minutes(10),
    ));

// New:
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

Requires `use tower_sessions::cookie::SameSite;` (verify
re-export path against tower-sessions 0.14 API at
implementation time).

### `config.rs` — token TTL ≥ session idle timeout

Add to `ServerConfig::validate()`:

```rust
const WEB_SESSION_IDLE_SECS: u64 = 7 * 24 * 3600; // 604800
if self.secrets.max_token_ttl_seconds < WEB_SESSION_IDLE_SECS {
    panic!(
        "config error: max-token-ttl-seconds ({}) must be \
         >= {} (web session idle timeout is 7 days)",
        self.secrets.max_token_ttl_seconds,
        WEB_SESSION_IDLE_SECS,
    );
}
```

### `auth/extractors.rs` — session cookie fallback

Modify `AuthUser::from_request_parts` to try the session when
no `Authorization` header is present:

```
1. Try Authorization: Bearer <token>
   → if present, existing path (API key or PASETO)
   → if absent, go to 1b

1b. Extract Session from request parts
    → read "paseto_token" from session data
    → if present, treat as raw PASETO token → step 3
    → if absent, return 401

2. API key path (prefix "cbsk_") — unchanged
3. PASETO validation — unchanged (same for header and session)
4. Load user + capabilities — unchanged
5. Return AuthUser
```

Implementation: extract `Session` via
`Session::from_request_parts(parts, state)` inside the
existing `from_request_parts`. The `Session` extractor returns
`Ok` even when no cookie is present (it creates an empty
session), so check for the `"paseto_token"` key to distinguish
"no session" from "session without token".

Add `use tower_sessions::Session;` to imports.

### `routes/auth.rs` — callback rewrite

**Remove:**
- `LoginQuery.cli_port: Option<u16>` field
- Session insert/read of `cli_port` in both login and callback
- `if let Some(port) = cli_port` branch (localhost JS redirect
  HTML)
- Server-rendered HTML token page (CLI without port)
- `/#token=<base64>` redirect (web flow)

**Replace callback response with:**

```rust
if client_type == "cli" {
    // CLI: redirect to UI with token in fragment
    let redirect_url = format!("/#cli-token={token_b64}");
    Ok(Redirect::temporary(&redirect_url).into_response())
} else {
    // Web: store token in session, extend TTL, redirect to /
    session
        .insert("paseto_token", &raw_token)
        .await
        .map_err(/* ... */)?;
    session.set_expiry(Expiry::OnInactivity(
        time::Duration::days(7),
    ));
    Ok(Redirect::temporary("/").into_response())
}
```

The `session.cycle_id()` call (session fixation prevention)
remains exactly where it is — before the response branch.

### `routes/auth.rs` — new logout endpoint

Add `POST /api/auth/logout` to the `oauth_routes` router
group (rate-limited via tower-governor, same as `/login` and
`/callback`):

```rust
let oauth_routes = Router::new()
    .route("/login", get(login))
    .route("/callback", get(callback))
    .route("/logout", post(logout))
    .layer(governor_layer);
```

Handler:

```rust
async fn logout(
    State(state): State<AppState>,
    session: Session,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Revoke the token if one is stored in the session
    if let Ok(Some(raw_token)) =
        session.get::<String>("paseto_token").await
    {
        let hash = paseto::token_hash(&raw_token);
        let _ = db::tokens::revoke_token(
            &state.pool, &hash,
        ).await;
    }

    // Flush the session (deletes server-side data + clears
    // the cookie via Set-Cookie with Max-Age=0)
    session.flush().await.map_err(|e| {
        tracing::error!("session flush failed: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to clear session",
        )
    })?;

    Ok(Json(serde_json::json!({"detail": "logged out"})))
}
```

This handler does NOT use `AuthUser` — the session may contain
an expired or revoked token. The user should be able to log
out regardless. Placed in `oauth_routes` for rate limiting.

### `routes/auth.rs` — guard on `/token/revoke` for cookie users

The existing `POST /api/auth/token/revoke` handler reads the
`Authorization` header directly to find the token to revoke.
After this change, a cookie-authenticated user calling this
endpoint would pass the `AuthUser` extractor (via session) but
then fail when the handler tries to read the missing header.

Add a guard at the top of `revoke_token`:

```rust
if headers.get("authorization").is_none() {
    return Err(auth_error(
        StatusCode::BAD_REQUEST,
        "use /api/auth/logout for web sessions",
    ));
}
```

### Import changes

- `extractors.rs`: add `use tower_sessions::Session;`
- `auth.rs`: add `use tower_sessions::Expiry;`, remove `Html`
  from `use axum::response::{Html, IntoResponse, ...};`, and
  remove `HeaderValue` from
  `use axum::http::{HeaderMap, HeaderValue, StatusCode};`.
  `HeaderMap` stays (used by `revoke_token`).

### Testable

- Web login: redirects to `/` with `cbsd_session` cookie set.
  Subsequent `/api/auth/whoami` returns 200 (cookie auth).
- CLI login: redirects to `/#cli-token=<base64>`. No cookie
  set.
- Logout: `POST /api/auth/logout` clears cookie, revokes
  token. Subsequent `/api/auth/whoami` returns 401.
- Bearer header still works (CLI and API key auth unchanged).
- Token revocation invalidates web session on next request.
- User deactivation invalidates web session on next request.
- `/token/revoke` returns 400 for cookie-authenticated users.
- Server refuses to start with `max_token_ttl_seconds` < 7d.

**~180 authored lines.**
