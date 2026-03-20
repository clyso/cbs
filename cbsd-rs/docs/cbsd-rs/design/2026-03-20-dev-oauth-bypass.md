# Dev Mode OAuth Bypass

## Problem

The OAuth login flow requires Google client secrets and a
round-trip to Google's servers. In development, operators
don't have (and shouldn't need) Google credentials just to
test the service locally.

## Solution

When `dev.enabled: true`, the login endpoint skips the
Google redirect and auto-redirects to the callback with
`seed.seed-admin` as the email identity. The full
session -> callback -> user creation -> PASETO token path
is exercised — only the Google round-trip is bypassed.

## How it works

1. `GET /api/auth/login?client=cli` — in dev mode, instead
   of redirecting to Google:
   a. Create the OAuth session state (same as production).
   b. Read `seed.seed-admin` from config. If not set,
      return 500 "dev mode requires seed-admin".
   c. Redirect to `/api/auth/callback?state={state}
      &dev_email={seed_admin}`.

2. `GET /api/auth/callback` — in dev mode, when
   `dev_email` query param is present:
   a. Validate the `state` parameter against the session
      (same CSRF check as production).
   b. Use `dev_email` as the user email instead of
      exchanging a Google authorization code.
   c. Set user name to the email prefix (before `@`).
   d. Everything else is identical: create/update user,
      create PASETO token, respond based on `client_type`
      (CLI gets HTML with token, web gets redirect).

3. `cbc login <url>` works unmodified — the browser opens,
   auto-redirects through the callback, shows the token
   page. Zero-click login in dev mode.

## What changes on the server

### `CallbackQuery` struct change

The dev redirect URL has no `code` parameter (there is no
Google authorization code to carry). The current struct
requires `code: String` — axum returns 422 before the
handler body runs.

Fix: make `code` optional, add `dev_email`:

```rust
#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: String,
    dev_email: Option<String>,
}
```

The dev branch checks `dev_email.is_some()` and skips the
code exchange. The production branch checks
`code.is_some()` and proceeds normally.

### `login` handler

Add a branch after session setup:

```rust
if config.dev.enabled {
    let email = config.seed.seed_admin
        .as_ref()
        .ok_or("dev mode requires seed-admin")?;
    let callback_url = format!(
        "/api/auth/callback\
         ?state={state}&dev_email={email}"
    );
    return redirect to callback_url;
}
// ... existing Google redirect
```

### `callback` handler

Add a branch before the Google code exchange:

```rust
if config.dev.enabled && params.dev_email.is_some() {
    let dev_email = params.dev_email.unwrap();
    let name = dev_email
        .split('@')
        .next()
        .unwrap_or(&dev_email);
    // Skip Google exchange — use dev_email/name
    // directly. Continue with user creation + token.
}
// ... existing Google code exchange (uses code.unwrap())
```

Note: dev mode bypasses the `allowed_domains` check
since `seed-admin` is operator-configured, not
user-supplied. If `seed-admin` uses an email outside
`allowed_domains`, dev login succeeds where production
would reject it. Acceptable in dev mode.

### `main.rs` — skip OAuth config loading in dev mode

`load_oauth_config` unconditionally opens and parses the
Google secrets file. In dev mode with no secrets file,
this panics at startup.

Fix: skip the load and create a dummy `OAuthState`:

```rust
let oauth = if config.dev.enabled {
    auth::oauth::OAuthState::dummy()
} else {
    auth::oauth::load_oauth_config(
        &config.oauth.secrets_file,
    ).expect("failed to load OAuth secrets")
};
```

`OAuthState::dummy()` returns a struct with empty
placeholder values. The dev login path never calls
`build_google_auth_url` or `exchange_code_for_userinfo`,
so the dummy values are never used.

### `config.rs` — skip OAuth validation in dev mode

`ServerConfig::validate()` currently checks that
`allowed_domains` is non-empty (or
`allow_any_google_account` is true). In dev mode, skip
this check — the OAuth config section can contain dummy
values or be entirely defaulted.

## What does NOT change

- Session state validation (CSRF protection exercised).
- User creation/update logic.
- PASETO token generation.
- Token storage in DB.
- `cbc login` client flow (unmodified).
- `cbc whoami` (works normally with the real token).
- All subsequent authenticated requests.

## Config

No new config fields. Uses existing:

- `dev.enabled: bool` — gates the feature.
- `seed.seed-admin: String` — the email used for dev
  login. Should be a valid email format (the name is
  derived from the prefix before `@`).

Both already exist and are typically set together.

## Security

The `dev_email` query parameter is only honored when
`dev.enabled: true`. In production (`dev.enabled: false`),
the parameter is ignored and the normal Google OAuth flow
runs.

The startup warning "DEVELOPMENT MODE — do not use in
production" (from Phase 7.5) already alerts operators.

## Scope

~50 lines of server changes:
- `CallbackQuery` struct: make `code` optional, add
  `dev_email` (~3 lines).
- `login` handler: dev branch after session setup
  (~10 lines).
- `callback` handler: dev branch before code exchange
  (~15 lines).
- `main.rs`: conditional OAuth loading (~5 lines).
- `auth/oauth.rs`: `OAuthState::dummy()` (~5 lines).
- `config.rs`: skip OAuth validation in dev mode
  (~5 lines).

No client changes. No migration. No new endpoints.
