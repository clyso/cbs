# Implementation Review: Security Audit Remediation — SecretString Wrap v3

| Field           | Value                                                                                                                                                                                                                                                                                                                      |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Series          | 019                                                                                                                                                                                                                                                                                                                        |
| Type            | impl                                                                                                                                                                                                                                                                                                                       |
| Version         | v3                                                                                                                                                                                                                                                                                                                         |
| Date            | 2026-05-31                                                                                                                                                                                                                                                                                                                 |
| Reviewer        | Staff Engineer (adversarial — no trust in implementer claims)                                                                                                                                                                                                                                                              |
| Commit in scope | `396b03cd` — `cbsd-rs: wrap in-memory token material in SecretString`                                                                                                                                                                                                                                                      |
| Design doc      | `cbsd-rs/docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md`                                                                                                                                                                                                                                              |
| Plan doc        | `cbsd-rs/docs/cbsd-rs/plans/019-20260516T1033-security-audit-remediation.md` (commit 14, lines 694–724)                                                                                                                                                                                                                    |
| Prior reviews   | `cbsd-rs/docs/cbsd-rs/reviews/019-20260530T1452-impl-security-audit-remediation-secret-wrap-v1.md`                                                                                                                                                                                                                         |
|                 | `cbsd-rs/docs/cbsd-rs/reviews/019-20260531T1525-impl-security-audit-remediation-secret-wrap-v2.md`                                                                                                                                                                                                                         |
| Build           | `SQLX_OFFLINE=true cargo build --workspace` — clean, 0 errors, 0 warnings. `SQLX_OFFLINE=true cargo test --workspace` — 260 tests pass (cbc: 8, cbsd-common: 5, cbsd-proto: 24, cbsd-server: 170, cbsd-worker: 53). `SQLX_OFFLINE=true cargo clippy --workspace --all-targets` — clean. `cargo fmt --all --check` — clean. |

---

## 1. Summary Assessment

Both v2 carry-forward actions are resolved: the `WorkerToken` `Debug` test now
exists and is genuine (negative + positive assertions), and the PASETO tracing
test has a positive control so a silent capture failure cannot produce a vacuous
pass. Every v1 finding was already confirmed resolved in v2, and this review
independently re-verified them. The production wrapping logic remains correct,
wire semantics are byte-identical, and the workspace builds and tests clean
under `--all-targets` clippy. The two `trybuild` compile-fail tests remain
ROADMAP-deferred — that deferral is honestly documented with a defensible
rationale and is unchanged from v2.

**Verdict: GO.** All prior findings are resolved or properly deferred. This
commit is ready to proceed to commit 15.

---

## 2. Strengths

**Both v2 carry-forward actions are fully resolved.** The
`worker_token_debug_redacts_api_key` test in `cbsd-proto/src/lib.rs` constructs
a `WorkerToken` with a known `api_key` value and asserts both that the raw value
is absent from `{:?}` output AND that `"<redacted>"` is present. The tracing
test in `paseto::tests` now has a positive control
(`logs_contain("emitting a wrapped token")`) so a subscriber-ordering failure
cannot mask a vacuous pass.

**`WorkerToken` test integrity is sound.** The test does not rely on `secrecy` —
`cbsd-proto` has no `secrecy` dependency, and the test is correct because it
exercises the hand-written `impl Debug for WorkerToken` directly. The
two-assertion structure (absent value AND present redaction placeholder) is the
minimum meaningful guard for a manual `Debug` impl.

**All v1 idiomatic-construction findings remain resolved.** Zero
`into_boxed_str()` calls exist in source. All construction sites use
`SecretString::from(s)` for `String` values and `SecretString::from ("literal")`
for `&str` literals. The `secrecy` workspace dependency (`secrecy = "0.10"` in
`[workspace.dependencies]`) is in place; all three consuming crates
(`cbsd-server`, `cbsd-worker`, `cbc`) use `secrecy.workspace = true`. Cargo.lock
confirms one resolved version (`0.10.3`).

**Wire semantics are byte-identical.** All `expose_secret()` call sites produce
the same byte sequences as the pre-wrap state. The 12 production call sites (2
in `admin.rs`, 2 in `auth.rs`, 2 in `robots.rs`, 1 in `connection.rs`, 1 in
`client.rs`, 1 in `config.rs` save path, 1 in `main.rs`) were verified against
surrounding context. The `save()` path still calls `set_permissions` for 0600
Unix mode at the expected location — that code was not disturbed by the diff.

**"OAuth" plan entry is resolved correctly.** The plan's
`cbsd-server (PASETO + OAuth - robot tokens)` scope means the OAuth callback
flow's PASETO `raw_token` is wrapped. Confirmed: `auth.rs` `callback()` wraps
via `SecretString::from(raw_token)` inside `token_create`, and every boundary in
that handler calls `raw_token.expose_secret()`. The Google OAuth client secret
is a config-time `String` in `config.rs` — the same class as the PASETO signing
key, both explicitly named out-of-scope in the commit message and the design.
There is no contradiction between plan and commit.

**`WorkerConfig` plain `api_key` deferral is correctly recorded.**
`WorkerConfig` at line 49 of `cbsd-worker/src/config.rs` derives
`#[derive(Debug, Deserialize)]` over `api_key: Option<String>` — the
pre-resolved config form. The ROADMAP (lines 144-145) records: "`cbsd- worker`'s
`WorkerConfig` derives `Debug` over a plain `api_key`; redact it as part of
commit 15's tracing/`Debug` sweep." This is a correct and properly documented
deferral.

**`trybuild` deferrals remain properly documented.** ROADMAP lines 43-51 name
both `trybuild` cases (plan commit-14 test items 2 and 3), state the rationale
(rustc `.stderr` fixtures are version-brittle), and reference the v1 review as
origin. Unchanged from v2.

**`allow-expose` annotation sweep deferral remains recorded.** ROADMAP lines
141-143 document that the 12 `expose_secret()` call sites in this commit will
need `// allow-expose` annotations when the CI grep gate lands. This is the
correct position: annotating before the gate exists provides no enforcement
value.

---

## 3. Blockers

None.

---

## 4. Serious Concerns

None.

---

## 5. Minor Issues

None remaining. All v2 minor issues (M1 and M2) are resolved.

---

## 6. Suggestions (Non-Blocking)

### N1 — `main.rs` uses fully-qualified `secrecy::SecretString::from`

`cbc/src/main.rs` line 147 writes `secrecy::SecretString::from(token)` without a
top-level `use secrecy::SecretString;` import. Every other call site in the
workspace follows the import-then-use pattern. This is valid Rust and not a bug,
but it is a minor style inconsistency. A follow-up `use` import at the top of
`main.rs` would align with the project's established pattern.

### N2 — PASETO signing key and OAuth client secret absent from ROADMAP

The PASETO signing key (`config.secrets.token_secret_key`) and the Google OAuth
client secret remain plain `String` in `cbsd-server/src/config.rs`. Design lines
895-896 list "PASETO key bytes" as a `Secret<T>` use case. These are config-time
secrets, not in-memory token holders, and are correctly outside this commit's
scope. The ROADMAP has no entry for them. A single ROADMAP line would close the
audit trail and prevent a future reviewer from treating the omission as an
oversight.

---

## 7. Open Questions

**OQ1 — PASETO signing key scope.** As noted in v2 OQ1: the ROADMAP has no entry
for wrapping the PASETO signing key or the Google OAuth client secret in
`Secret<T>`. This does not block commit 14 — the in-memory-token-holder scope is
correct and the design supports treating config-time secrets separately. But
without a ROADMAP entry, the audit trail is incomplete. This is the same open
question from v2, still unresolved.

---

## 8. Prior-Iteration Findings Resolution

### v2 findings

| v2 Finding                                   | Severity | Resolution in v3 | Notes                                                                                                                       |
| -------------------------------------------- | -------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------- |
| M1: `impl Debug for WorkerToken` has no test | Minor    | **RESOLVED**     | Test added in `cbsd-proto/src/lib.rs`; negative + positive assertions; passes (`worker_token_debug_redacts_api_key ... ok`) |
| M2: tracing-test lacks positive control      | Minor    | **RESOLVED**     | `logs_contain("emitting a wrapped token")` added as second assertion in `secret_token_is_redacted_in_tracing_output`        |

### v1 findings (re-verified independently)

| v1 Finding                              | Severity   | Status in v3       | Notes                                                                            |
| --------------------------------------- | ---------- | ------------------ | -------------------------------------------------------------------------------- |
| D4: M1 — non-idiomatic `into_boxed_str` | Minor      | **RESOLVED**       | Zero `into_boxed_str` in source; all sites use `SecretString::from`              |
| N2: N1 — `secrecy` not a workspace dep  | Suggestion | **RESOLVED**       | `secrecy = "0.10"` in `[workspace.dependencies]`; 3 crates on `workspace = true` |
| S2: `WorkerToken` overclaim             | Serious    | **RESOLVED**       | Doc-comment present; commit message scoped; design option (b) satisfied          |
| S1: tracing-test absent                 | Serious    | **RESOLVED**       | Test present; passes; genuine (not vacuous)                                      |
| S1: trybuild absent, no ROADMAP         | Serious    | **RESOLVED**       | ROADMAP lines 43–51 document deferral with defensible rationale                  |
| M2: allow-expose                        | Minor      | Carried to ROADMAP | ROADMAP lines 141–143 record the sweep for when the gate lands                   |
| M3: `WorkerConfig` `Debug`              | Minor      | Carried to ROADMAP | ROADMAP lines 144–145; addressed in commit 15 sweep                              |
| OQ1: signing key scope                  | Open Q     | Still open         | Not in ROADMAP; does not block                                                   |

---

## 9. Confidence Score

| Item                              | Points | Description                                                                                                                                                                                                                                                                                                                                                                                                                      |
| --------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                    | 100    |                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| D1: Two `trybuild` tests deferred | -20    | Plan lines 722–724 required two compile-fail tests (item 2: `#[derive(Serialize)]` over `SecretString`; item 3: inner-field access without `.expose_secret()`). Both are deferred as a single mechanism for the same documented reason — rustc `.stderr` fixtures are version-brittle — with explicit ROADMAP entry (lines 43–51). Scored as one -20, consistent with v2, justified by identical mechanism and shared rationale. |
| **Total**                         | **80** |                                                                                                                                                                                                                                                                                                                                                                                                                                  |

### Interpretation

**80/100 — Acceptable with noted improvements; fix before next stage.**

The step from 65 (v2) to 80 (v3) represents the resolution of the two v2
carry-forward actions: the `WorkerToken` `Debug` test (D5 → resolved) and the
tracing-test positive control (D11 → resolved). The single remaining deduction
is the two `trybuild` tests that the plan explicitly required but are now
ROADMAP-deferred. That deferral is honest, documented, and carries a defensible
rationale.

**The commit is ready to proceed to commit 15.** No further actions are required
before proceeding.
