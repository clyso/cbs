# Implementation Review: Web UI Authentication (v1)

**Commit:** `97d7bcf cbsd-rs/server: add BFF session cookie auth for web UI`
**Design:** `015-20260325T0040-web-ui-authentication.md`
**Plan:** `015-20260325T0129-web-ui-authentication.md`
**Reviewer:** Claude (automated)
**Date:** 2026-03-25

## Verdict: Ready to Merge

The implementation is clean, correct, and closely follows the plan.
Compiles without warnings. Good code deduplication via the
`validate_paseto()` extraction. All findings are low severity.

---

## Commit Boundary

Single commit: 192 insertions, 112 deletions (net +80). Passes all
five smell tests.

| Test | Pass | Evidence |
|------|------|---------|
| One-sentence purpose | Yes | "Add BFF session cookie auth for web UI users" |
| Previous commit compiles | Yes | `506aa08` is the design docs commit |
| Revertable | Yes | Entire BFF feature in one commit |
| Testable | Yes | Web login, CLI login, logout, bearer auth, TTL validation |
| No dead code | Yes | Every addition has callers in this commit |

---

## Findings

### F1 (Low) — Logout swallows `session.get()` errors silently

`routes/auth.rs:469`:

```rust
if let Ok(Some(raw_token)) = session.get::<String>("paseto_token").await {
```

If `session.get()` returns `Err(...)`, the error is silently
discarded. The session is still flushed (correct), but the token
remains unrevoked until its TTL expires. No log is emitted for the
error case.

**Recommendation:** Use a `match` and log the error:

```rust
match session.get::<String>("paseto_token").await {
    Ok(Some(raw_token)) => {
        let hash = paseto::token_hash(&raw_token);
        if let Err(e) = db::tokens::revoke_token(&state.pool, &hash).await {
            tracing::warn!("logout: failed to revoke session token: {e}");
        }
    }
    Ok(None) => {} // no token stored — nothing to revoke
    Err(e) => tracing::warn!("logout: could not read session token: {e}"),
}
```

---

### F2 (Low) — No debug logging for auth fallthrough path

`extractors.rs:262–282`: When the Bearer header is absent and the
session has no `paseto_token`, the extractor returns 401 with no
log at any level. The old code logged
`warn!("auth reject: missing or invalid Authorization header")`.

The silence is intentional — web UI makes many unauthenticated
requests (loading the SPA, login page) that would produce noise at
`warn`. However, no logging at all makes it harder to diagnose
"why doesn't my request authenticate?"

**Recommendation:** Add `trace!` or `debug!` for the transition
points:

```rust
// After bearer_result fails:
tracing::debug!("auth: no bearer header, trying session cookie");

// When session has no token:
tracing::debug!("auth reject: no bearer header and no session token");
```

---

### F3 (Low) — Warn-to-debug log level change is unmentioned

The old code used `tracing::warn!` for successful auth path events:


- "auth: processing token"
- "auth: PASETO decoded successfully"
- "auth: token valid, loading user"

The new code correctly downgrades these to `tracing::debug!`. This
is the right fix (success events are not warnings), but it's a
behavioral change: operators filtering at `warn` level will no
longer see per-request auth flow logs. Not mentioned in the commit
message.

**Impact:** Low — these logs were incorrectly noisy at `warn`. The
change is an improvement. No action needed unless the commit
message is amended.

---

## Code Quality

### Positive: `validate_paseto()` extraction

The PASETO validation logic (decode → hash → revocation check →
load user) was previously inline in `from_request_parts`. The new
`validate_paseto()` function (`extractors.rs:177–219`) is shared by
both the Bearer and session paths. This is a clean deduplication
that reduces the extractor's complexity and prevents logic drift
between the two auth paths.

### Positive: Shared `WEB_SESSION_IDLE_SECS` constant

`config.rs:20`: The constant is defined once and used in both the
config validation (`config.rs:269`) and the session expiry
(`routes/auth.rs:333`). This prevents the magic-number duplication
the plan review warned about.

### Positive: Session lifecycle ordering

The callback correctly calls `session.cycle_id()` (line 303)
before storing the token in the session (line 323), preventing
session fixation. The session already has its new ID when the token
is inserted.

---

## Verified Against Plan

| Plan item | Status |
|-----------|--------|
| `main.rs`: explicit cookie attributes (`with_name`, `with_http_only`, `with_same_site`, `with_path`, `with_secure`) | Done (`main.rs:156–163`) |
| `config.rs`: `WEB_SESSION_IDLE_SECS` const + validation in `ServerConfig::validate()` | Done (`config.rs:20–26`, `config.rs:266–274`) |
| `extractors.rs`: Bearer → Session fallback with `Session::from_request_parts` | Done (`extractors.rs:229–286`) |
| `extractors.rs`: `validate_paseto()` shared helper | Done (`extractors.rs:177–219`) |
| `routes/auth.rs`: remove `cli_port` from `LoginQuery`, login, callback | Done (field, session insert, session read, HTML branches all removed) |
| `routes/auth.rs`: CLI redirects to `/#cli-token=` | Done (`auth.rs:316`) |
| `routes/auth.rs`: web stores token in session + extends TTL | Done (`auth.rs:322–334`) |
| `routes/auth.rs`: logout in `oauth_routes` (rate-limited) | Done (`auth.rs:67`) |
| `routes/auth.rs`: logout handler with session flush + token revocation | Done (`auth.rs:464–486`) |
| `routes/auth.rs`: `revoke_token` guard for cookie users | Done (`auth.rs:375–381`) |
| Import cleanup: `Html`/`HeaderValue` removed, `HeaderMap` kept | Done (`auth.rs:15–16`) |
| `Session` import added to extractors | Done (`extractors.rs:23`) |
| `WEB_SESSION_IDLE_SECS` imported in auth.rs | Done (`auth.rs:32`) |
| Compiles | Yes (`cargo check` passes) |

---

## Deferred / Not In Scope

No TODOs or deferred items in the implementation. All design-level
deferrals (PKCE, refresh tokens, multi-tab, SSE/WS) remain
unchanged — none were supposed to be addressed in this commit.

---

## Summary

| ID | Severity | Title |
|----|----------|-------|
| F1 | Low | Logout swallows `session.get()` errors silently |
| F2 | Low | No debug logging for auth fallthrough path |
| F3 | Low | Warn-to-debug log level change unmentioned in commit message |

All findings are low severity. The implementation is correct, well-
structured, and ready to merge.
