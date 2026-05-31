# Implementation Review: Security Audit Remediation — SecretString Wrap v2

| Field           | Value                                                                                                                       |
| --------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Series          | 019                                                                                                                         |
| Type            | impl                                                                                                                        |
| Version         | v2                                                                                                                          |
| Date            | 2026-05-31                                                                                                                  |
| Reviewer        | Staff Engineer (adversarial — no trust in implementer claims)                                                               |
| Commit in scope | `f113d23b` — `cbsd-rs: wrap in-memory token material in SecretString`                                                       |
| Design doc      | `cbsd-rs/docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md`                                               |
| Plan doc        | `cbsd-rs/docs/cbsd-rs/plans/019-20260516T1033-security-audit-remediation.md` (commit 14, lines 694–724)                     |
| Prior review    | `cbsd-rs/docs/cbsd-rs/reviews/019-20260530T1452-impl-security-audit-remediation-secret-wrap-v1.md`                          |
| Build           | `SQLX_OFFLINE=true cargo build --workspace` — clean, 0 errors, 0 warnings. `SQLX_OFFLINE=true cargo test --workspace` — 256 |
|                 | tests pass (cbc: 8, cbsd-common: 5, cbsd-proto: 23, cbsd-server: 170, cbsd-worker: 53). `SQLX_OFFLINE=true cargo clippy     |
|                 | --workspace`— clean.`cargo fmt --all --check` — clean.                                                                      |

---

## 1. Summary Assessment

The v1 required actions are fully satisfied: the non-idiomatic
`SecretString::new(s.into_boxed_str())` construction is gone, `secrecy` is
promoted to a workspace dependency, the commit message accurately scopes the F13
guarantee and names `WorkerToken` as a deliberate plain DTO, a `tracing-test`
redaction test is present and genuine (not vacuous), and the two `trybuild`
compile-fail tests are now formally ROADMAP-deferred with a defensible rationale
(`.stderr` fixture brittleness). One new carry-forward gap emerged during v2
review: the manual `impl Debug for WorkerToken` — the highest- risk new code in
this commit — has no test, while every other redaction mechanism introduced in
this commit (cbc `Config`, PASETO `SecretString`) received a test. This is
inconsistent with the project's own pattern. It is not a blocker, but it must be
either tested or ROADMAP-deferred before this series is considered clean.

**Verdict: accept with one carry-forward action.** Production code and boundary
semantics are correct. The commit may proceed to commit 15 after either (a)
adding a 5-line `Debug`-redaction test for `WorkerToken` in `cbsd-proto`, or (b)
adding a ROADMAP line for it alongside the existing trybuild deferral.

---

## 2. Strengths

**All v1 idiomatic-construction findings fully resolved.** Every construction
site now uses `SecretString::from(s)` (for a `String`) or
`SecretString::from("literal")` (for a `&str`). Zero `into_boxed_str()` calls
remain in source — confirmed by exhaustive grep. The `raw_clone` pattern in
`generate_token_material` (clone moved into the Argon2 hasher, original `raw`
consumed by `SecretString::from`) is pre-existing and correct.

**Workspace dependency consolidation is complete.** `secrecy = "0.10"` is in
`[workspace.dependencies]`; all three crates (`cbsd-server`, `cbsd-worker`,
`cbc`) use `secrecy.workspace = true`. Cargo.lock shows exactly one resolved
version (`0.10.3`) — no diamond-dependency splits.

**Commit message accurately represents coverage.** The reworded message names
`WorkerToken` as a deliberate transport-only DTO, explicitly scopes the F13
by-construction guarantee to "in-memory holders," and does not overclaim.

**`WorkerToken` doc-comment is correct and complete.** The doc-comment on
`cbsd_proto::WorkerToken` accurately describes the plain-DTO decision, points to
the in-memory secret holder (`ResolvedWorkerConfig.api_key: SecretString`), and
explicitly prohibits `Display` or log-path leakage. The design option (b)
("separate the wire DTO from the in-memory secret holder") is satisfied.

**Manual `Debug for WorkerToken` correctly redacts `api_key`.** The hand-
written impl formats `api_key` as the string literal `"<redacted>"`, passes all
other fields through their own `Debug` implementations, and uses `finish()` (not
`finish_non_exhaustive()`). There is no `Display` path on `WorkerToken` that
could leak the field.

**`tracing-test` redaction test is genuine.** The test constructs a
`SecretString`, emits it through a real `tracing::info!` call with the `?`
format specifier (the `%`/Display path does not exist — `Secrecy 0.10.3`
implements `Debug` but not `Display` for `SecretBox<str>`, so `?` is the forced,
correct adaptation), and asserts the inner value is absent from captured output.
Confirmed against `secrecy 0.10.3` source: `Debug for SecretBox<S>` emits
`SecretBox<str>([REDACTED])`. The negative assertion is the security- relevant
one and is not vacuous.

**cbc `Config` has both a `Debug`-redaction test and a
`static_assertions::assert_not_impl_any!(Config: Serialize)` guard.** The round-
trip test independently verifies the on-disk JSON format via `serde_json::Value`
inspection and confirms the token survives a `save`/`load` cycle with the
correct value. The structural `Serialize` guard is acknowledged to test only the
current type, not a general class — this is documented and honest.

**`trybuild` deferrals are now properly documented.** ROADMAP lines 43–51
explicitly name both `trybuild` cases (D10/F13), note that commit 14 shipped the
`tracing-test` redaction test and the `static_assertions` guard, and give a
concrete and defensible rationale (`.stderr` fixtures are
rustc-version-brittle). This is materially different from v1, where the
rationale was the incorrect "bin crates cannot be trybuilt." The deferral is now
honest.

**Wire semantics are byte-identical at every boundary.** All 12
`expose_secret()` sites produce the same byte sequences as pre-wrap:

- `auth.rs` base64 encode: `raw_token.expose_secret().as_bytes()` → same bytes.
- `auth.rs` session insert: `raw_token.expose_secret()` is `&str`; serializes to
  the same JSON string.
- `admin.rs` / `robots.rs` one-time responses: `.expose_secret().to_string()` is
  identity.
- `connection.rs` Bearer header: format string output is unchanged.
- `client.rs` Bearer header: same.

**`ResolvedWorkerConfig` correctly omits `Debug`.** The resolved config struct
(which holds `api_key: SecretString`) has no `#[derive(Debug)]` — adding one
would require a manual impl or would fail to compile. The pre-resolved
`WorkerConfig` (which derives `Debug` over `api_key: Option<String>`) is a
pre-existing condition addressed in the ROADMAP under the commit 15 tracing
sweep.

---

## 3. Blockers

None. The v1 required actions are satisfied.

---

## 4. Serious Concerns

None.

---

## 5. Minor Issues

### M1 — `impl Debug for WorkerToken` has no test; inconsistent with project pattern

The manual `Debug` impl in `cbsd-proto/src/lib.rs` is the highest-risk new code
in this commit: hand-written redaction is more error-prone than a derive, and it
is in a public shared crate. Every other redaction mechanism introduced in this
commit received a test:

- cbc `Config.token` → `config::tests::token_is_redacted_in_debug`
- `SecretString` in tracing →
  `paseto::tests::secret_token_is_redacted_in_tracing_output`

`WorkerToken` has neither a `Debug`-redaction test nor a ROADMAP entry for one.
A future developer could refactor the manual impl (e.g., add a `Display` derive,
or replace `&"<redacted>"` with `&self.api_key` by accident), and the regression
would go undetected. The test is five lines:

```rust
#[test]
fn worker_token_api_key_is_redacted_in_debug() {
    let t = WorkerToken { worker_id: "w1".into(), worker_name: "n".into(),
                          api_key: "cbsk_secret".into(), arch: "x86_64".into() };
    let rendered = format!("{t:?}");
    assert!(!rendered.contains("cbsk_secret"), "api_key must not appear in Debug: {rendered}");
}
```

**Resolution:** Either add this test to `cbsd-proto/src/lib.rs` (strongly
preferred, 5 lines), or add an explicit ROADMAP line alongside the existing
trybuild deferral.

### M2 — Tracing-test lacks a positive control

`secret_token_is_redacted_in_tracing_output` asserts the negative: the raw value
is absent from captured output. It does not assert the positive: that capture is
working at all. If `#[traced_test]` silently stopped capturing (e.g., due to a
subscriber ordering issue), the negative assertion would still pass. A single
additional line resolves this:

```rust
assert!(logs_contain("emitting a wrapped token"),
        "tracing capture must be active for this test to be meaningful");
```

This is a nit: the test is not currently vacuous (confirmed by reading the
`tracing-test` and `secrecy` source), but the positive control is defensive and
cheap.

---

## 6. Suggestions (Non-Blocking)

### N1 — `%` vs `?` in tracing for `SecretString` — a design-doc clarification

The design (line 958) states: `tracing::debug!(token = %my_secret, ...)`. This
example cannot compile — `secrecy 0.10.3` does not implement `Display` for
`SecretBox<str>`. The correct specifier is `?` (Debug), which is what the test
uses. The discrepancy is benign (the design's intent is correct, the
implementation is correct, and the compile-time guard is stronger than
intended), but a follow-up note in the design or a comment in the test would
prevent confusion for future contributors.

### N2 — `expose_secret()` call sites have no `// allow-expose` annotation

The design (line 945) specifies a CI grep gate that will reject
`.expose_secret()` calls lacking `// allow-expose`. The 12 call sites introduced
in this commit are not annotated. When the gate lands, a retroactive sweep will
be required. The ROADMAP (lines 142–143) already records this. No action needed
now.

---

## 7. Open Questions

**OQ1 — PASETO signing key and OAuth client secret not in ROADMAP.** The PASETO
signing key (`config.secrets.token_secret_key`) and the OAuth client secret
remain plain `String` in `cbsd-server/src/config.rs`. Design lines 895–896 list
"PASETO key bytes" as a `Secret<T>` use case; design lines 856–857 enumerate
token types and do not list the signing key, supporting the out-of- scope
decision. The ROADMAP has no entry for these. This does not block the current
commit (the commit-14 scope is in-memory token holders, not config-time
secrets), but a ROADMAP line would close the audit trail.

---

## 8. v1 Findings Resolution Checklist

| v1 Finding                      | Severity   | Resolution in v2   | Notes                                                                     |
| ------------------------------- | ---------- | ------------------ | ------------------------------------------------------------------------- |
| D4: M1                          | Minor      | **RESOLVED**       | All 7 sites now use `SecretString::from(s)`; zero `into_boxed_str` in src |
| N2: N1                          | Suggestion | **RESOLVED**       | `secrecy` in `[workspace.dependencies]`; 3 crates on `workspace = true`   |
| S2: WorkerToken overclaim       | Serious    | **RESOLVED**       | Doc-comment added; commit message scoped; option (b) satisfied            |
| S1: tracing-test absent         | Serious    | **RESOLVED**       | Test present in `paseto::tests`; passes                                   |
| S1: trybuild absent, no ROADMAP | Serious    | **RESOLVED**       | ROADMAP lines 43–51 document deferral with defensible rationale           |
| M2: allow-expose                | Minor      | Carried to ROADMAP | ROADMAP lines 142–143 record the sweep for when the gate lands            |
| M3: WorkerConfig Debug          | Minor      | Carried to ROADMAP | ROADMAP lines 144–145; addressed in commit 15 sweep                       |
| OQ1: signing key scope          | Open Q     | Still open         | Not in ROADMAP; does not block                                            |

---

## 9. Confidence Score

| Item                                          | Points | Description                                                                                                                     |
| --------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                | 100    |                                                                                                                                 |
| D1: Two `trybuild` tests deferred             | -20    | Plan lines 722–724 required two compile-fail tests; now ROADMAP-deferred with an honest rationale (rustc `.stderr` brittleness) |
| D5: `impl Debug for WorkerToken` has no test  | -10    | Hand-written redaction code; every other redaction in this commit was tested; pattern inconsistency                             |
| D11: Missing positive control in tracing-test | -5     | Negative-only assert; silent capture failure would pass the test undetected                                                     |
| **Total**                                     | **65** |                                                                                                                                 |

### Interpretation

**65/100 — Significant issues; address before proceeding.**

The major change from v1 (35/100) is that two of the three v1 blockers — the
production correctness issue (`WorkerToken` overclaim, D7) and the absent
`tracing-test` (one D1) — are now resolved. The remaining D1 deduction is
unchanged in nature (two `trybuild` tests deferred), but is now properly
documented and not a dropped deliverable. The new D5 deduction
(`WorkerToken Debug` untested) is the sole new finding.

**To reach the 75+ range:**

1. **Required before "done" (carries from v1):** Add a 5-line test in
   `cbsd-proto` asserting `format!("{:?}", worker_token)` does not contain the
   `api_key` value — OR add an explicit ROADMAP line for it alongside the
   trybuild deferral. If deferred to ROADMAP, this commit is acceptable to
   proceed to commit 15.
2. **Recommended:** Add a positive control assertion to
   `secret_token_is_redacted_in_tracing_output`
   (`logs_contain("emitting a wrapped token")`). One line.

With action 1 completed, score rises to approximately 80.
