# Design Review: 01 — Authentication (v2)

**Verdict: Approved.**

All v1 concerns resolved: token TTL corrected to 24h default,
`cli_port` explicitly deferred, scope visibility note added
to `whoami`, 401 message uses stored `host`.

No blockers. No major concerns.

## Minor Issues

- **`whoami` response field name.** Verify the server's
  `/api/auth/whoami` response struct field is
  `effective_caps` (not `caps`). A mismatch would cause
  the caps line to be empty.

- **Login URL `?client=cli` semantics.** The server silently
  ignores unknown query params. Worth a note that `client=cli`
  is the mechanism that triggers the copy-paste flow.

## Strengths

- Token TTL now accurately documented as 24h.
- `cli_port` scope deferral is honest and clean.
- Preserving `host` on 401 is good UX.
- Scope visibility limitation noted with pointer to
  `admin users get`.
