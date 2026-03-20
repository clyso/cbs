# Design Review: Dev Mode OAuth Bypass (v2)

**Document:**
`docs/cbsd-rs/design/009-20260320T0750-dev-oauth-bypass.md`

---

## Summary

Both v1 blockers are resolved. The `CallbackQuery` struct
now has `code: Option<String>` and `dev_email: Option<String>`.
The `main.rs` OAuth loading is conditional with
`OAuthState::dummy()`. Config validation skip is documented.

No blockers. No major concerns.

**Verdict: Approved.**

---

## Prior Findings Disposition

| v1 Finding | Status |
|---|---|
| B1 — `CallbackQuery.code` required | Resolved |
| B2 — `load_oauth_config` panic | Resolved |

---

## Blockers

None.

---

## Major Concerns

None.

---

## Minor Issues

- **Production `code` path uses `code.unwrap()`.** After the
  dev branch, the production path needs `code` to be `Some`.
  The design shows `code.unwrap()` — this will panic if a
  production callback arrives without `code` (e.g., a
  malformed redirect from Google). Use
  `code.ok_or(auth_error(400, "missing code"))?` instead
  of `unwrap()`. This is an implementation note, not a
  design flaw.

---

## Strengths

- Both v1 blockers resolved with clean, minimal fixes.
- Full auth path exercised in dev mode (session, CSRF,
  user creation, PASETO).
- `cbc login` works unmodified — zero-click in dev.
- No new config fields.
- Security gate (`dev.enabled`) is correct.
- Domain bypass honestly documented.
- Scope is minimal (~50 lines).
