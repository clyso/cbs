# Phase 11: Dev Mode OAuth Bypass

**Design:**
`docs/cbsd-rs/design/2026-03-20-dev-oauth-bypass.md`

## Progress

| # | Commit | ~LOC | Status |
|---|--------|------|--------|
| 1 | `cbsd-rs/server: add dev mode OAuth bypass` | ~50 | TODO |

**Total:** ~50 LOC, 1 commit.

---

## Why one commit

The feature is ~50 lines across 4 files. Every piece
depends on the others: the login redirect needs the
callback to accept `dev_email`, which needs
`OAuthState::dummy()` to exist, which needs config
validation to skip OAuth checks. No useful intermediate
state exists.

---

## Commit 1: `cbsd-rs/server: add dev mode OAuth bypass`

**Files:**

- `cbsd-server/src/routes/auth.rs`
- `cbsd-server/src/main.rs`
- `cbsd-server/src/auth/oauth.rs`
- `cbsd-server/src/config.rs`

### `routes/auth.rs`

**`CallbackQuery` struct:** Make `code` optional, add
`dev_email`:

```rust
pub struct CallbackQuery {
    code: Option<String>,
    state: String,
    dev_email: Option<String>,
}
```

**`login` handler:** After session setup, before Google
redirect:

```rust
if state.config.dev.enabled {
    let email = state.config.seed.seed_admin
        .as_ref()
        .ok_or_else(|| auth_error(
            500, "dev mode requires seed-admin",
        ))?;
    let url = format!(
        "/api/auth/callback\
         ?state={oauth_nonce}&dev_email={email}"
    );
    return Ok(Redirect::temporary(&url).into_response());
}
```

**`callback` handler:** Before Google code exchange:

```rust
if state.config.dev.enabled
    && params.dev_email.is_some()
{
    let email = params.dev_email.unwrap();
    let name = email
        .split('@')
        .next()
        .unwrap_or(&email)
        .to_string();
    // Skip domain check + Google exchange.
    // Jump to user creation with (email, name).
}
```

Production path: change `params.code` to
`params.code.ok_or_else(|| auth_error(400, "missing
code"))?` — not `unwrap()`.

### `auth/oauth.rs`

Add `OAuthState::dummy()`:

```rust
impl OAuthState {
    pub fn dummy() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: String::new(),
        }
    }
}
```

Fields depend on `OAuthState`'s actual structure — adapt
to whatever fields it has. The dummy values are never
used (dev mode never calls Google exchange functions).

### `main.rs`

Conditional OAuth loading:

```rust
let oauth = if config.dev.enabled {
    auth::oauth::OAuthState::dummy()
} else {
    auth::oauth::load_oauth_config(
        &config.oauth.secrets_file,
    ).expect("failed to load OAuth secrets")
};
```

### `config.rs`

In `ServerConfig::validate()`, skip the OAuth domain
check when dev mode is enabled:

```rust
if !self.dev.enabled {
    if self.oauth.allowed_domains.is_empty()
        && !self.oauth.allow_any_google_account
    {
        panic!("config error: ...");
    }
}
```

### Verification

```bash
SQLX_OFFLINE=true cargo build --workspace
SQLX_OFFLINE=true cargo test --workspace
```

Manual test: start server with `dev.enabled: true` and
`seed-admin: dev@local`, no OAuth secrets file. Open
`/api/auth/login?client=cli` in browser — should
auto-redirect and show token page.
