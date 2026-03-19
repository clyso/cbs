# Design Review: 01 — Authentication

**Document:** `docs/cbc/design/01-20260318T1801-authentication.md`

---

## Summary

The authentication command design is mostly correct. Two issues
need resolution: the token TTL documentation is wrong, and the
`cli_port` local-redirect flow is entirely absent.

**Verdict: Approve with conditions.**

---

## Blockers

None.

---

## Major Concerns

### M1 — Token TTL claim is definitively wrong

The design states: "Tokens are PASETO v4, issued by the server
with a configurable TTL (default: infinite)."

The server creates tokens with a hardcoded 24-hour expiry
(`let default_ttl: i64 = 86400` in `auth.rs`). There is no
code path that produces an infinite-TTL token. Operators will
be surprised when daily `cbc login` is required.

**Fix:** Change to: "Tokens have a server-configured TTL
(default 24 hours). The client does not manage expiry — on
401, the user must `cbc login` again."

### M2 — `cli_port` OAuth flow is entirely absent

The server's `GET /api/auth/login` accepts `cli_port: Option<u16>`.
When present, the OAuth callback redirects to
`http://localhost:{port}/callback?token=...` instead of
displaying the token for copy-paste. This avoids clipboard
interaction — better UX for interactive terminals.

The design describes only the copy-paste variant. The
`cli_port` flow requires the client to bind a local HTTP
listener before opening the browser.

**Fix:** Either document the `cli_port` flow as a mode
(`cbc login --auto` or default behavior) or explicitly state
it is out of scope for the first iteration.

### M3 — `whoami` output silently omits scopes

The server's `WhoamiResponse` returns roles as names only.
But users with `builds:create` scoped to specific channels
will not see their scope restrictions in `whoami` output.
An operator debugging access issues will miss the scope
constraint.

**Fix:** Either show scopes (requires a follow-up
`GET /api/permissions/users/{email}/roles`) or add a note:
"Scopes are not shown here — use
`cbc admin users get <email>` for full role details."

---

## Minor Issues

- **401 message should include stored host.** The saved config
  has `host`. Print `"session expired — run 'cbc login {host}'"`.

- **`open` crate should be in dependencies list.** Doc 00 does
  not list it, but doc 01 references `open::that()` for browser
  opening.

---

## Strengths

- Not parsing PASETO tokens client-side is correct.
- Preserving config `host` on 401 is good UX.
- Validating the token via `whoami` before saving prevents
  storing invalid credentials.
