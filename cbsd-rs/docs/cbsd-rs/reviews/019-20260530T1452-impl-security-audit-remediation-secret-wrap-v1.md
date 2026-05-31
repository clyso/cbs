# Implementation Review: Security Audit Remediation — SecretString Wrap v1

- **Series:** 019
- **Type:** impl
- **Version:** v1
- **Date:** 2026-05-30
- **Reviewer:** Staff Engineer (adversarial — no trust in implementer claims)
- **Commit in scope:** `7ebe6846` —
  `cbsd-rs: wrap in-memory token material in SecretString`
- **Design doc:**
  `cbsd-rs/docs/cbsd-rs/design/019-20260514T1040-security-audit-remediation.md`
- **Plan doc:**
  `cbsd-rs/docs/cbsd-rs/plans/019-20260516T1033-security-audit-remediation.md`
  (commit 14, lines 694–724)
- **Build verification:** `SQLX_OFFLINE=true cargo build --workspace` — clean, 0
  errors, 0 warnings. `SQLX_OFFLINE=true cargo test --workspace` — all tests
  pass. `SQLX_OFFLINE=true cargo clippy --workspace` — clean.
  `cargo fmt --all --check` — clean.

---

## 1. Summary Assessment

The production wrapping logic is technically correct: every in-memory secret
holder that this commit touches now carries a `SecretString`, the secrecy 0.10
API is used correctly, boundary semantics are byte-identical to the pre-wrap
state, and the workspace builds and tests clean. However, the commit ships
without the tests its own plan requires — and without any ROADMAP entry
deferring them. The plan-mandated `tracing-test` redaction test and the two
`trybuild` compile-fail tests are entirely absent. The D9 ROADMAP deferral
(`249fbe48`) covers only the URI-logging assertions from commits 7–13; it does
not cover commit 14's secret-redaction tests. Absent a documented deferral,
these are **dropped deliverables**, not deferred ones.

Additionally, `cbsd-proto::WorkerToken.api_key` is left as a plain `String`
despite the plan explicitly listing `cbsd-proto` as an in-scope package. The
commit message asserts the F13 by-construction guarantee is now in effect, but
`WorkerToken` already derives `Serialize` with a plain `api_key` field — the
guarantee does not extend to this type. The commit message overclaims.

**Verdict: conditional accept.** The production logic is sound. Before this
commit is considered done, the implementer must either (a) add a ROADMAP entry
documenting the dropped test deliverables with a trigger condition, or (b)
implement the tests. The `WorkerToken` gap must be addressed with either a
proper wrap or an explicit doc-comment mirroring `ConfigFile`'s rationale, plus
a softened commit message.

---

## 2. Strengths

**Correct secrecy 0.10 usage.** `SecretString` is `SecretBox<str>`; construction
via `SecretString::new(s.into_boxed_str())` is functionally correct.
`ExposeSecret` is a trait; every call site properly imports
`use secrecy::ExposeSecret;`. `expose_secret()` returns `&str`. The `serde`
feature is not enabled, so `SecretString` has no `Serialize` impl. `Clone` is
implemented — no clone issues introduced.

**Byte-identical wire semantics.** Every `.expose_secret()` boundary site was
verified to produce the same value as the pre-wrap code:

- `auth.rs` base64 encoding: `raw_token.as_bytes()` →
  `raw_token.expose_secret().as_bytes()`. Identical bytes.
- `auth.rs` session insert: was `&raw_token: &String`, now
  `raw_token.expose_secret(): &str`. Both serialize to the same JSON string.
- `connection.rs` Bearer header: format string produces the same
  `"Bearer <key>"` string.
- `client.rs` Bearer header: same.
- `admin.rs`/`robots.rs` one-time-reveal responses:
  `.expose_secret().to_string()` round-trips cleanly.

**cbc config compatibility preserved.** The `Config`/`ConfigFile` split cleanly
separates in-memory holder (`Config { token: SecretString }`) from the on-disk
DTO (`ConfigFile { token: String }`). The JSON format — top-level `host` and
`token` fields, token in plaintext — is unchanged. File permissions (0600) are
still set by the same `set_permissions` call.

**Debug redaction is correct.** `SecretBox<str>::Debug` outputs
`SecretBox<str>([REDACTED])`. The `token_is_redacted_in_debug` test correctly
asserts the raw value is absent from the rendered string.

**Save/load round-trip test is solid.** The test independently verifies the
on-disk format via `serde_json::Value` inspection, not just the round-trip
value, providing format-regression protection.

**No new tracing leak introduced.** All traced values after this commit were
verified: no tracing site in the changed files emits a wrapped `SecretString`
via `Display`/`Debug`/`%`/`?` format specifiers. Commit 15 (log-site redaction)
remains the correct scope for pre-existing log sites.

---

## 3. Serious Concerns

### S1 — Three plan-required tests absent with no documented deferral

The plan at lines 722–724 requires:

1. 1 `tracing-test` redaction test asserting `token=<redacted>` in captured log
   output.
2. 1 `trybuild` compile-fail test for `#[derive(Serialize)]` over a
   `Secret<String>` field.
3. 1 `trybuild` compile-fail test for inner-field access without
   `.expose_secret()`.

None are present. The implementer substituted
`static_assertions::assert_not_impl_any!(Config: Serialize)` for item 2, arguing
bin crates cannot be used by trybuild fixtures. This argument is incorrect: a
trybuild compile-fail fixture defines its own types in its own source file and
never imports the crate under test. A fixture containing
`struct Foo { x: secrecy::SecretString } #[derive(serde::Serialize)] struct Bar { foo: Foo }`
compiles against a lib crate (e.g., `cbsd-proto`) without needing to import
`cbc`. The substitution also tests a weaker property:
`assert_not_impl_any!(Config: Serialize)` is structurally guaranteed as long as
`token` is `SecretString`, so it cannot detect a future regression where a
wrapper struct around `Config` gains Serialize by a different path. The plan's
`trybuild` tests cover a general class of misuse; `assert_not_impl_any!` covers
only the specific current struct.

The `tracing-test` redaction test (item 1) is the most important: it verifies
the central behavioral claim of the commit — that `SecretString` causes log
sinks to emit `<redacted>` rather than the raw value when a developer adds
`tracing::debug!(token = %raw)`. Without it, the redaction guarantee has no
regression harness. This test belongs in `cbsd-server` (the primary
secret-holder crate) and requires adding `tracing-test` as a dev-dependency.

The existing ROADMAP deferral at commit `249fbe48` covers three tests from
commits 7–13 (D9 URI-logging policy). It explicitly names "tracing-test
log-capture assertions for the URI redaction policy (audit-rem D9)." It does not
mention commit 14's secret-wrapping tests. The ROADMAP also lacks any entry
covering the `trybuild` or `tracing-test` items for D10/F13.

**Resolution:** Either add a ROADMAP entry that explicitly names these three
tests, distinguishes them from the D9 URI-logging deferral, and provides a
concrete trigger condition — or implement them now. The `tracing-test` redaction
test is the priority; it is the behavioral guarantee the commit claims to
provide.

### S2 — `cbsd-proto::WorkerToken.api_key` left plain; commit message overclaims

The plan explicitly lists `cbsd-proto (WorkerToken.api_key)` as in-scope. The
commit leaves `WorkerToken` unchanged. The design at lines 931–938 states: "Wire
types in `cbsd-proto` that today contain raw token strings (`api_key`, the
various `*Token` fields) must be migrated to `Secret<String>`."

The commit message asserts: "a token-bearing in-memory struct can no longer gain
`#[derive(Serialize)]` by accident — the wrap half of audit-rem D10 / F13
by-construction guarantee." This claim is false for `WorkerToken`: it already
derives both `Serialize` and `Debug`, and holds `api_key: String`. The
by-construction guarantee is not universal; it has a named exception the commit
does not acknowledge.

Runtime risk is bounded: `WorkerToken` is assembled in `build_worker_token()`
and immediately serialized to JSON; no tracing site emits it. The worker
deserializes it and immediately moves `api_key` into `SecretString` at lines
243/280 of config.rs. But the Debug path exists (`#[derive(Debug)]` on a struct
with `api_key: String`) and will leak if a future developer adds
`tracing::debug!(?payload, ...)` to `build_worker_token()`.

**Resolution:** Choose one:

(a) Wrap properly: add a `SecretString` field to `WorkerToken`, provide a custom
`Serialize` impl that calls `.expose_secret()` for the `api_key` field, and
update the worker's deserialization site. This mirrors what the plan describes
for the "Option A" migration path.

(b) Document the plain-DTO decision: add a doc-comment on `WorkerToken`
explaining the deliberate choice (mirroring `ConfigFile`'s doc comment in cbc)
and update the commit message to reflect that `WorkerToken` is an
intentionally-plain transport DTO and is explicitly excluded from the
by-construction invariant.

Either (a) or (b) is acceptable, but (b) must be paired with softening the
commit message claim from "the wrap half of the F13 by-construction guarantee"
to an accurate statement of actual coverage.

---

## 4. Minor Issues

### M1 — Non-idiomatic `SecretString` construction

All wrapping sites use `SecretString::new(s.into_boxed_str())`. The secrecy 0.10
source documents `From<String>` as "the preferred method for construction"
(lib.rs line 213). The idiomatic form is `SecretString::from(s)` or equivalently
`s.into()`. Both are functionally identical; the `into_boxed_str()` path is not
wrong, only non-idiomatic.

### M2 — `expose_secret()` call sites lack `// allow-expose` annotations

The design (lines 944–952) specifies a CI grep gate (deferred to ROADMAP) that
rejects `.expose_secret()` calls lacking `// allow-expose` on the same line.
This commit introduces 12 `.expose_secret()` call sites, none annotated. When
the gate lands (or when any contributor runs the future grep script), all 12
sites will need retroactive annotation. Adding the annotations now costs 12
single-line edits and prevents a future sweeping fixup commit.

### M3 — `WorkerConfig` derives `Debug` with `api_key: Option<String>` exposed

`WorkerConfig` (the YAML-deserialized config, before `resolve()`) derives
`Debug` at line 49 of config.rs and holds `api_key: Option<String>`. If
`WorkerConfig` is ever emitted via `{:?}` in an error or startup log, the key
appears in plaintext. This is a pre-existing condition not created by this
commit, but commit 15 (log-site redaction) should address it.

---

## 5. Suggestions

### N1 — `static_assertions` can be moved to integration test or lib test

`assert_not_impl_any!(Config: Serialize)` lives in a `#[cfg(test)]` block in
`cbc/src/config.rs`. Since `SecretString` structurally prevents `Serialize`, the
assertion is nearly tautological. If it must remain, consider placing it
alongside an explanatory comment that makes clear it tests the structural
guarantee for the current type only — not a general property of the wrapping
scheme.

### N2 — Consider workspace-level `secrecy` dependency

Three crates (`cbc`, `cbsd-server`, `cbsd-worker`) now independently pin
`secrecy = "0.10"`. Promoting this to `[workspace.dependencies]` in the root
`Cargo.toml` ensures all crates stay on the same minor version and simplifies
future upgrades. The plan mentioned workspace `Cargo.toml` as a touchpoint but
the commit only added per-crate pins.

---

## 6. Open Questions

**OQ1 — Signing key and OAuth secret scope.** `config.secrets.token_secret_key`
(the PASETO signing key) and the OAuth client secret remain plain `String` in
`cbsd-server/src/config.rs`. The commit message states these are "out of scope."
Design lines 856–857 enumerate "bearer token, PASETO raw token, session token,
robot token, or API key" and do not list the signing key, supporting
out-of-scope. However, design lines 895–896 list "PASETO key bytes" as an
intended `Secret<T>` use case. If the signing key is intentionally deferred, it
should be listed in the ROADMAP alongside the test deferrals.

---

## 7. Confidence Score

| Item                                                                       | Points | Description                                                                                                                |
| -------------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                             | 100    |                                                                                                                            |
| D1: `tracing-test` redaction test absent, no ROADMAP deferral              | -20    | Central behavioral guarantee for commit 14 has no test; not a documented deferral                                          |
| D1: Two `trybuild` compile-fail tests absent, no ROADMAP deferral          | -20    | Plan items 2 and 3 for commit 14; substitute `static_assertions` is weaker and does not cover the general-property test    |
| D7: `WorkerToken.api_key` plain despite plan listing `cbsd-proto` in scope | -20    | By-construction guarantee does not hold for this type; commit message overclaims; future `tracing::debug!` path would leak |
| D4: Non-idiomatic `SecretString::new(s.into_boxed_str())` across 7 sites   | -5     | `From<String>` is the documented preferred form                                                                            |
| **Total**                                                                  | **35** |                                                                                                                            |

---

### Interpretation

**35/100 — Significant issues. Must address before proceeding.**

The production wrapping logic is sound and byte-identical at every boundary. The
score is not about correctness of what was shipped but about completeness: the
commit's central claim — that the F13 by-construction guarantee is now in effect
— has no behavioral test harness and is not fully accurate for `WorkerToken`.
The test gap is the most urgent action item. With the three missing tests added
(or formally ROADMAP-deferred) and the `WorkerToken` situation addressed (either
wrapped or documented), the score would be in the 80–85 range.

**Required actions before proceeding to commit 15:**

1. Either implement the `tracing-test` redaction test and the two `trybuild`
   compile-fail tests, or add a ROADMAP entry that explicitly names all three
   (distinct from the existing D9 URI-logging entry) with a concrete trigger
   condition.
2. Either wrap `WorkerToken.api_key` in `SecretString` (with a custom
   `Serialize` impl) or add a `ConfigFile`-style doc-comment explaining the
   deliberate plain-DTO decision **and** revise the commit message to not claim
   a universal by-construction guarantee.
