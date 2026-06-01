# Review — Security Audit Remediation: Tracing Redaction (v1)

| Field    | Value                                                                       |
| -------- | --------------------------------------------------------------------------- |
| Seq      | 019                                                                         |
| Type     | impl                                                                        |
| Commit   | `26e5debf` — `cbsd-rs/server: redact token material from auth-path tracing` |
| Phase    | Phase 2, commit 15 of the security-audit-remediation series                 |
| Reviewer | Claude Sonnet 4.6 (adversarial, v1)                                         |
| Date     | 2026-06-01                                                                  |
| Verdict  | **NO-GO — one blocker must be fixed before merge**                          |

---

## 1. Summary Assessment

The commit correctly implements a per-process non-reversible diagnostic
identifier (`token_diag_id`) and redacts the four `extractors.rs` auth-path
sites and three create/rotate log sites. The helper's cryptographic construction
is sound: a `LazyLock` process-lifetime random salt fed into salted SHA-256,
truncated to 16 hex chars; it is non-reversible and non-correlatable across
processes.

One blocker prevents the stated deliverable from being true: the
`DELETE /api/auth/api-keys/{prefix}` revoke handler at `routes/auth.rs:739`
still logs `(prefix={})` with the raw lookup prefix. The commit message and body
claim "Closes SI-10 ('no token material in any log line')" — that claim is false
with this site standing. The commit also exhibits internal inconsistency: the
three sibling creation handlers were redacted, but the revoke handler was not.
The correct fix is not to compute a diag-id from the prefix (the handler holds
no plaintext token for that), but to drop the `prefix=...` field from the log
line entirely; `user.email` already identifies the actor and `prefix` in the log
is not needed for diagnosis.

---

## 2. Strengths

**Cryptographic construction of `token_diag_id` is correct.** The `LazyLock`
salt is a genuine 16-byte random value initialized once per process via
`rand::thread_rng().gen()`. SHA-256 over `salt ‖ token` produces a 256-bit
digest; only the first 16 hex chars (64-bit prefix) are surfaced. An attacker
with log access and a candidate token cannot reproduce the id without the
process-private salt.

**Correlation is sound.** Because `token_diag_id` hashes the full raw bearer
string, the same token produces the same id wherever it appears in a single
process run. The create-path sites (`routes/auth.rs:629-633`,
`routes/robots.rs:336-341`, `routes/robots.rs:664-672`) call `.expose_secret()`
on the just-created plaintext and hash it; the extractors sites hash the
identical bearer string on every auth validation. The ids from these two code
paths will match within a session.

**All four `extractors.rs` sites are correctly redacted.** Both
`token_str[..20]` raw-prefix sites and both `hash[..16]` hash-prefix sites are
replaced with `token_id = %token_cache::token_diag_id(...)`. The `hash` binding
(line 224) is retained for DB revocation lookup and `mark_token_used` — it is
not leaked to logs and its continued presence is correct.

**No unused binding introduced.** After the `hash_prefix` removal, `hash` at
line 224 still participates in two live DB calls. Clippy confirms this:
`cargo clippy --workspace` exits clean with zero warnings.

**Full build and test suite passes.** `cargo check`, `cargo clippy`, and
`cargo test` all pass with `SQLX_OFFLINE=true`. Total: 262 tests, 0 failures, 0
warnings.

**Out-of-scope exclusions are correctly identified.** The worker `config.rs:222`
warn logs only a static disambiguation message (no token value). The cbc robot
`println!` at `admin/robots.rs:458,843` are intentional CLI stdout (one-time
token display to the operator, not logs). Both exclusions are correctly
justified.

---

## 3. Blockers

### B1 — `revoke_api_key_handler` still logs the raw lookup prefix

**File**: `cbsd-rs/cbsd-server/src/routes/auth.rs`, line 739.

```rust
tracing::info!("revoked API key (prefix={}) for {}", prefix, user.email);
```

**What the problem is.** This `info!` line logs `prefix` — the raw 12-hex-char
lookup prefix from the URL path `DELETE /api/auth/api-keys/{prefix}`. The commit
redacted the identical `(prefix=…)` field in the three sibling handlers (create
API key, create-or-revive robot, create-or-rotate token) but left this site
untouched.

**Why it matters.** The commit subject and body explicitly state "Closes SI-10
('no token material in any log line')." With this site intact, that claim is
false. More precisely, the commit is internally inconsistent: it takes the
position that `prefix` is token material at the three create sites (replacing it
with a diag-id) while simultaneously logging it in the clear at the revoke site.
One of these positions must be wrong.

**Practical sensitivity context.** The prefix is non-secret by design: it is
returned in `CreateApiKeyResponse.prefix` and `ApiKeyItem.prefix`, it is the URL
path parameter, and it is stored plaintext in `key_prefix`. It is also already
emitted by the TraceLayer via `Uri::path()` on each
`DELETE /api/auth/api-keys/<prefix>` request. The raw information exposure risk
is therefore low. However, the commit's entire job is completeness; leaving any
"prefix" field in a post-commit log line falsifies the stated deliverable.

**Correct fix.** The handler holds only `prefix` (a URL path param), not the
plaintext token, so `token_diag_id(prefix)` would compute an id uncorrelatable
with any auth log line — a meaningless artefact. The right change is to drop the
field entirely:

```rust
tracing::info!("revoked API key for {}", user.email);
```

`user.email` already identifies the actor; the prefix is already in the
access-log path from `Uri::path()`.

**Root cause.** Plan commit-15 specifies "targeted `tracing-test` assertions per
fixed site." The implemented tests exercise only the `token_diag_id` helper in
isolation, not any real handler log line. A per-site `#[traced_test]` on the
revoke path would have caught this immediately.

---

## 4. Major Concerns

### M1 — Tests exercise the helper only; no call-site coverage

The commit adds two unit tests:

1. `token_diag_id_is_stable_and_distinct` — verifies stability and length.
2. `token_diag_id_does_not_leak_token_in_tracing` — calls
   `tracing::warn!(token_id = %token_diag_id(raw), "auth reject")` in a
   synthetic context and asserts the raw value is absent.

Neither test fires any of the seven real log sites that were changed. The
`#[traced_test]` directly exercises a synthetic `warn!` — it does not go through
`validate_paseto`, the bearer extractor, or any of the `routes/` handlers. A bug
at one of those real sites (as B1 demonstrates) is invisible to the current test
suite.

Plan commit-15 says "targeted `tracing-test` assertions **per fixed site**."
That was not implemented. The correct remediation is at least one
`#[traced_test]` on the actual auth path or one real handler, asserting the raw
token value is absent from captured output.

Severity: major (not a separate blocker because B1 is the concrete miss; this is
the systemic cause). Fixing B1 without adding per-site tests would leave the
policy unenforced against future regressions.

---

## 5. Minor Issues

### N1 — `CachedToken` and `CandidateRow` derive `Debug` without redaction

`CachedToken` (line 56) holds `prefix: String`. `CandidateRow` (line 210) holds
`hash: String` (an Argon2id PHC string) and `prefix: String`. Both have
`#[derive(Debug)]`. The D10 "construction- tight" policy covers `Debug` impls,
not only `tracing!` call sites.

Today neither struct is ever Debug-formatted into a log or format string, so
there is no active leak. The risk is latent: a future
`tracing::debug!(entry = ?cached, ...)` would silently emit both the prefix and
the argon2 hash.

The correct long-term fix — per D10 construction-tight — is to either: (a)
remove the `Debug` derive and replace with a hand-rolled impl that redacts
`prefix` and `hash`, or (b) add a `#[derive(Debug)]` guard test asserting the
Debug output does not contain a real prefix or hash value.

This is not a blocker for commit 15 alone, but it is a gap left open by the
"construction-tight" claim.

### N2 — Commit message claims workspace-scope but scope is server-only

The commit subject is
`cbsd-rs/server: redact token material from auth-path tracing`. This matches the
actual scope. The message body says "Replace … the lookup-prefix in the API-key
and robot-token creation logs" — accurate. No cross-workspace sweep was
performed (nor needed, since the only token-material tracing sites across the
workspace are in `cbsd-server`), so the scope claim and the body are consistent.

No change needed; this is a note for the reviewer record.

---

## 6. Suggestions

### S1 — Note in commit body that prefix is non-secret by design

The open question below (OQ1) asks the team to decide whether `prefix` is
secret. Once that decision is explicit, it would help to add a comment near
`create_api_key_handler` and `revoke_api_key_handler` noting the chosen policy —
e.g., "prefix is a non-secret routing aid; it is intentionally not redacted from
response bodies or URL paths but is excluded from tracing fields per SI-10
consistency."

### S2 — Per-site `#[traced_test]` template for future call sites

The design document specifies a CI grep gate (deferred to ROADMAP) to catch new
`tracing!` sites that emit token material. Until that gate lands, a per-site
`#[traced_test]` on at least the validate_paseto hot path would provide the same
property in the interim.

---

## 7. Open Questions

### OQ1 — Is the `prefix` field token material or not?

The commit treats `prefix` as token material in create/rotate logs (replacing
it) but not in the revoke log (retaining it). The design's D10 policy does not
explicitly include or exclude lookup prefixes — it covers "any bearer token,
PASETO raw token, session token, robot token, or API key."

A lookup prefix is derived from the first 12 hex chars of a 64-hex-char random
key. It is publicly returned in API responses, stored plaintext, and already
logged via `Uri::path()` on every `DELETE /api/auth/api-keys/{prefix}` request.
The practical argument for treating it as non-secret is strong.

The team should decide once: if prefix is non-secret, the three create- path
redactions were unnecessary overhead (though harmless); if it is secret, the
access-log path (`Uri::path()`) also needs to be addressed. Either way, the
codebase should be consistent.

---

## 8. Confidence Score

| Item                                                                             | Points | Description                                                             |
| -------------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------- |
| Starting score                                                                   | 100    |                                                                         |
| D7: raw prefix logged at revoke site                                             | -20    | `routes/auth.rs:739` falsifies the SI-10 close                          |
| D5: no per-site call-site tests                                                  | -15    | Tests cover helper in isolation only; plan required per-site assertions |
| D10: CachedToken/CandidateRow Debug derives expose prefix/hash without redaction | -5     | Construction-tight D10 claim left partially open                        |
| **Total**                                                                        | **60** |                                                                         |

**Range interpretation**: 50–74 — Significant issues. Must address before
proceeding.

---

## 9. Verdict

**NO-GO.** Blocker B1 (`revoke_api_key_handler` logging raw prefix) must be
fixed and the plan's per-site test requirement (M1) should be addressed before
this commit is considered to close SI-10. The fix is small — drop the `prefix=`
field from one `info!` call — but the inconsistency is a direct contradiction of
the stated deliverable.

Required actions before re-review:

1. Remove `prefix={}` from `revoke_api_key_handler` line 739.
2. Add at least one `#[traced_test]` on a real auth-path handler (not just the
   helper) asserting the raw token is absent.

Optional (ROADMAP):

1. Audit `CachedToken` and `CandidateRow` `Debug` derives for construction-tight
   compliance.
2. Document the prefix secret/non-secret decision so create-path and revoke-path
   are consistent.
