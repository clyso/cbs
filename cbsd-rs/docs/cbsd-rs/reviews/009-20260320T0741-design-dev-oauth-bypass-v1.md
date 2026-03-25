# Design Review: Dev Mode OAuth Bypass

**Document:**
`docs/cbsd-rs/design/009-20260320T0750-dev-oauth-bypass.md`

**Cross-referenced against:**


- `cbsd-server/src/routes/auth.rs`
- `cbsd-server/src/config.rs`
- `cbsd-server/src/main.rs`
- `cbsd-server/src/auth/oauth.rs`

---

## Summary

The design is clean and well-scoped — two branches in
`auth.rs` plus one validation skip in `config.rs`. The
approach of exercising the full session → callback → user
→ token path while only bypassing Google is correct. However,
2 blockers must be fixed: the callback's `CallbackQuery`
struct requires a `code` field which the dev redirect doesn't
provide, and `main.rs` unconditionally calls
`load_oauth_config` at startup which will panic if the
secrets file doesn't exist.

**Verdict: Approve with conditions — fix the 2 blockers.**

---

## Blockers

### B1 — `CallbackQuery.code` is required, dev bypass omits it

The callback handler deserializes query params via:

```rust
#[derive(Deserialize)]
pub struct CallbackQuery {
    code: String,   // required
    state: String,
}
```

The design's dev redirect URL is:

```
/api/auth/callback?state={state}&dev_email={email}
```

No `code` parameter. Axum will return 422 (Failed to
deserialize query string) before the handler body
executes. The dev bypass logic never runs.

**Fix:** Make `code` optional in `CallbackQuery`:

```rust
pub struct CallbackQuery {
    code: Option<String>,
    state: String,
    dev_email: Option<String>,
}
```

Then the dev branch checks `dev_email.is_some()` and
skips the code exchange. The production path checks
`code.is_some()` and proceeds normally. Both are
gated by `config.dev.enabled`.

### B2 — `load_oauth_config` panics at startup without secrets file

`main.rs:107-108`:

```rust
let oauth = auth::oauth::load_oauth_config(
    &config.oauth.secrets_file,
).expect("failed to load OAuth secrets");
```

This runs unconditionally. In dev mode, the design says
"the `oauth` config section can be omitted or contain
dummy values." But `load_oauth_config` opens and parses
the file — a missing or dummy file causes a panic at
startup.

**Fix:** The design already identifies this:
"skip the OAuth file existence check when `dev.enabled`."
Implement as:

```rust
let oauth = if config.dev.enabled {
    auth::oauth::OAuthState::dummy()
} else {
    auth::oauth::load_oauth_config(
        &config.oauth.secrets_file,
    ).expect("failed to load OAuth secrets")
};
```

Where `OAuthState::dummy()` returns a struct with empty
placeholder values. The dev login path never calls
`build_google_auth_url` or `exchange_code_for_userinfo`,
so the dummy values are never used.

Alternatively, make `secrets_file` optional in
`OAuthConfig` when `dev.enabled` is true.

---

## Minor Issues

- **Domain restriction check in callback.** The production
  callback checks `allowed_domains` against the Google
  email (line 245-264). The dev bypass skips this check
  since there's no Google response. This is correct —
  `seed_admin` is configured by the operator, not
  user-supplied. But note that the dev email bypasses
  domain restrictions. If `seed_admin` is set to an email
  outside `allowed_domains`, the dev login succeeds while
  production login would reject that email. Acceptable in
  dev mode but worth a comment.

- **`seed_admin` must be a valid email format.** The
  callback's name derivation (`email.split('@').next()`)
  produces the username prefix. If `seed_admin` is not
  email-formatted (e.g., just `"admin"`), the name will
  be the entire string and the email won't match any user
  created via production OAuth. This is not a bug — just
  a configuration constraint worth noting.

---

## Strengths

- **Full auth path is exercised.** Session state, CSRF
  validation, user creation, PASETO token generation,
  and token storage all run in dev mode. Only the Google
  round-trip is skipped. This means dev mode tests the
  real auth stack.

- **`cbc login` works unmodified.** Zero client changes.
  The browser auto-redirects through the callback, shows
  the token page. Zero-click login.

- **No new config fields.** Reuses `dev.enabled` and
  `seed.seed_admin` which already exist.

- **Security gate is correct.** `dev_email` is only
  honored when `dev.enabled: true`. The startup warning
  from Phase 7.5 alerts operators.

- **Scope is minimal.** ~40 lines of changes. Two
  branches and one validation skip.
