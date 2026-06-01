# Review — Security Audit Remediation: Tracing Redaction (v2)

| Field    | Value                                                                       |
| -------- | --------------------------------------------------------------------------- |
| Seq      | 019                                                                         |
| Type     | impl                                                                        |
| Commit   | `40bba7db` — `cbsd-rs/server: redact token material from auth-path tracing` |
| Phase    | Phase 2, commit 15 of the security-audit-remediation series                 |
| Reviewer | Claude Sonnet 4.6 (adversarial, v2)                                         |
| Date     | 2026-06-01                                                                  |
| Verdict  | **GO — all v1 blockers resolved; one doc-consistency minor outstanding**    |

---

## 1. Summary Assessment

This is the amended v2 of commit 15. The v1 blocker (B1: revoke handler logging
prefix while sibling create handlers used `token_diag_id`, producing an
internally inconsistent policy) is resolved by the maintainer ruling that the
12-char lookup prefix is a non-secret index. The amended commit reverts the
three create/rotate lifecycle sites back to logging the prefix directly, so all
four key-lifecycle log lines (`create_api_key`, robot `create`/`revive`, robot
token `rotate`, `revoke_api_key`) now consistently log the non-secret prefix.
The auth-path sites (`extractors.rs`) correctly continue to redact the raw
presented token via `token_diag_id`. The v1 M1 finding (test coverage limited to
helper in isolation) is addressed with two new `#[traced_test]` integration
tests that drive real auth-path code and are confirmed non-vacuous by mutation.
The v1 N1 finding (latent `Debug` on `CandidateRow`) is addressed with a
hand-written `Debug` that redacts `hash`. One minor finding remains: the D10
design document text says "no portion of any bearer token…or API key may be
written to logs at any level" but the ruling now permits the non-secret prefix
in lifecycle logs. The design doc needs a carve-out added. This does not block
the commit.

---

## 2. Strengths

**All four v1 required actions addressed, none on faith.** B1 is resolved by a
principled policy decision (prefix is non-secret) rather than a surface
workaround. The fix is stable: the ruling makes the four lifecycle sites
consistent, not merely compliant by accident.

**`token_diag_id` cryptographic construction verified correct.** The
`LazyLock<[u8; 16]>` salt is generated once per process via
`rand::thread_rng().r#gen()`. SHA-256 over `salt ‖ token` produces a 256-bit
digest; the first 16 hex chars (64-bit prefix) are surfaced. This is
non-reversible from a log entry alone (salt is process-private), and
non-correlatable across restarts. The `r#gen()` raw-identifier syntax is
required because `gen` is a reserved keyword in Rust 2024; correct. The
`hex_encode(&digest)[..16]` slice is safe: SHA-256 always produces 32 bytes → 64
hex chars, so `[..16]` is never out-of-range.

**Four extractors.rs auth-path sites all redacted.** `token_str[..20]`
raw-prefix sites (lines 218, 276) and `hash[..16]` hash-prefix sites (lines
228, 245) are replaced with `token_id = %token_diag_id(token_str)` (now using
`token_cache::token_diag_id`). The `hash` binding at line 224 remains live for
DB revocation lookup and `mark_token_used` — not a dead binding, not a leak.

**Two genuine integration tests, confirmed non-vacuous.** Both tests
(`bearer_auth_does_not_log_raw_token` and
`paseto_decode_failure_does_not_log_raw_token`) exercise real code paths: the
first drives `AuthUser::from_request_parts` with a real in-memory DB and a
correctly-shaped (69-char) but unregistered `cbsk_` API key; the second drives
`validate_paseto` directly with a bogus PASETO token. The `#[traced_test]`
subscriber installs at TRACE level, so `tracing::debug!` events are captured.
Mutation testing (temporarily reverting line 276 to the old
`token_prefix = &token_str[..20]` form) caused
`bearer_auth_does_not_log_raw_token` to fail with a log line containing
`deadbeef` — confirming the test is not vacuous. The revert restores the pass.

**`CandidateRow` Debug redacts `hash`.** The hand-written `Debug` for
`CandidateRow` (token_cache.rs) formats `hash` as `"<redacted>"` and shows only
the non-secret `prefix`. The two source row types (`ApiKeyRow` in
`db/api_keys.rs` and `TokenCandidate` in `db/robots.rs`) do not derive `Debug`,
eliminating the latent Argon2-hash exposure path that v1 identified.

**`WorkerToken` Debug is also safe.** `cbsd_proto::WorkerToken` has a
hand-written `Debug` that formats `api_key` as `"<redacted>"` and is covered by
a `#[traced_test]` unit test. `WorkerToken` is only used at one site in
production code (admin.rs:341) where it is JSON-serialized directly into the
response body (not Debug-formatted). No `{:?}` log site exists for it.

**Full build and test suite passes.** `cargo clippy --workspace` exits clean
with zero warnings. Workspace-wide `cargo test` reports 264 tests (8 + 5 + 24 +
174 + 53), 0 failures, 0 warnings against `SQLX_OFFLINE=true`.

**Aggregate redaction policy is coherent.** Under the maintainer ruling:

- Raw presented token bytes → `token_diag_id` (auth path, extractors.rs)
- Argon2 credential hash → `"<redacted>"` in CandidateRow Debug; source structs
  lack Debug derive entirely
- Lookup prefix → logged in plain at lifecycle sites (ruling: non-secret)
- Worker API key → `"<redacted>"` in WorkerToken Debug; redacted in
  `SecretString` on the PASETO create path

---

## 3. Blockers

None.

---

## 4. Major Concerns

None. All v1 major concerns are resolved.

---

## 5. Minor Issues

### N1 — Design document D10 text contradicts the prefix ruling

**File**:
`cbsd-rs/docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md`,
lines 856–857.

**What the problem is.** D10 as written states: "No portion of any bearer token,
PASETO raw token, session token, robot token, or API key may be written to logs
at any level." The 12-char lookup prefix is a portion of the API key and robot
token. The four key-lifecycle log sites now log it in plain. Under the
maintainer ruling this is correct, but the design document text does not reflect
the ruling.

**Why it matters.** The design document is the authoritative record. A future
reviewer (or CI gate) reading D10 verbatim would flag the lifecycle log sites as
non-compliant. The ruling should be captured in the design doc as a named
carve-out, e.g., "The 12-char lookup prefix is a non-secret routing index: it is
stored plaintext, returned in API responses, and appears in URL paths. Logging
it at lifecycle sites is permitted under this policy; only raw key bytes and
Argon2 credential hashes are secret material that must never appear in logs."

**Resolution direction.** Add the carve-out to D10 in the design document (in a
fixup to the design-doc commit, per project convention).

This was OQ1 from v1 — the question has been answered by the maintainer; the
answer just needs to be written down.

### N2 — `CachedToken` still derives `Debug`, but is genuinely safe

`CachedToken` at `token_cache.rs:56` still has `#[derive(Debug)]`. Its fields
are: `kind` (enum, safe), `token_id` (DB row ID, not a token byte),
`owner_email` (non-secret identity), `prefix` (non-secret under the ruling),
`expires_at` (timestamp). Under the ruling no field is secret. The v1 N1 concern
about `CandidateRow` is resolved; this residual derive on `CachedToken` is not a
risk under the current policy. Noted for completeness: if the prefix ruling were
ever reversed, this derive would need attention.

---

## 6. Suggestions

### S1 — Robot token revoke path has no test coverage

`DELETE /api/admin/robots/{name}/token` (`revoke_robot_token_handler` in
`robots.rs`) is not exercised by the new tests. The log line at `robots.rs:741`
("user {} revoked {revoked} token(s) for robot '{name}'") does not log any
secret material, so this is not an active leak — but the plan's "per fixed site"
test requirement wasn't fully satisfied for lifecycle sites (only auth-path
sites got tests). Given the low risk, this is a suggestion rather than a
concern.

### S2 — `token_diag_id` call in `token_cache.rs` test is synthetic

The test `token_diag_id_does_not_leak_token_in_tracing` (token_cache.rs) calls
`tracing::warn!` directly in test context rather than going through a real use
site. This is fine as a unit test of the helper itself, but it does not protect
against a future call site emitting the raw token in a different field. The two
new integration tests in extractors.rs (which go through real code) provide the
stronger coverage. No action required; the combination is adequate.

---

## 7. Open Questions

### OQ1 — Plan text vs. implementation (minor discrepancy)

Plan commit-15 (line 744) says "API key prefix logging at debug (F13's original
site) is replaced with a stable per-process diagnostic identifier derived from
the key hash." This describes the original v1 approach. The amended commit
instead reverts the lifecycle sites to logging prefix and applies the diag-id
only to auth-path sites. The plan text is now slightly inaccurate as a
description of what commit 15 does (though the net SI-10 outcome is correctly
achieved). A fixup to the plan describing the ruling would be consistent, but
given that the plan tracks phase progress rather than individual implementation
decisions, this is low priority.

---

## 8. Confidence Score

| Item                                                      | Points | Description                                                                                                           |
| --------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------------------- |
| Starting score                                            | 100    |                                                                                                                       |
| D10: design doc text not updated to reflect prefix ruling | -5     | D10 text says "no portion…may be written to logs" but lifecycle prefix logging is now permitted — doc needs carve-out |
| **Total**                                                 | **95** |                                                                                                                       |

**Range interpretation**: 90–100 — Ready to merge. Minor or no issues.

---

## 9. v1 Finding Disposition

| Finding                                                                  | v1 Severity   | Status                                                                                                                                                                                          |
| ------------------------------------------------------------------------ | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| B1: revoke handler logging prefix while sibling sites used diag-id       | Blocker       | **RESOLVED** — maintainer ruling: prefix is non-secret; all four lifecycle sites now consistently log it; auth-path sites still use diag-id for raw token                                       |
| M1: tests covered helper in isolation, not real call sites               | Major         | **RESOLVED** — two `#[traced_test]` integration tests added; mutation-confirmed non-vacuous                                                                                                     |
| N1: CandidateRow/CachedToken Debug exposes hash/prefix without redaction | Minor         | **RESOLVED** — CandidateRow has hand-written Debug redacting hash; CachedToken's derived Debug is safe because no field is secret under the ruling; source row types lack Debug derive entirely |
| N2: commit message scope note                                            | Minor         | **NOTE ONLY** — no change needed; included here for completeness                                                                                                                                |
| OQ1: is prefix secret or not?                                            | Open question | **RESOLVED** — maintainer ruled prefix is non-secret; design doc carve-out needed (see N1 above)                                                                                                |

---

## 10. Verdict

**GO.** The commit is ready to merge. The v1 blocker and both v1 major concerns
are fully resolved. The single outstanding issue (N1: design doc D10 text needs
a prefix carve-out) is a documentation reconciliation that can be addressed in a
fixup to the design-doc commit before landing the branch. It does not block
merge of this commit.

Post-merge action required:

1. Add a carve-out to D10 in the design document: "The 12-char lookup prefix is
   a non-secret routing index and may appear in lifecycle log lines. Raw key
   bytes and Argon2 credential hashes are secret material and must never appear
   in logs."
