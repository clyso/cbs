# cbsd-rs Roadmap

Forward-looking items deferred from current implementation work. Each entry
records the motivation, the affected component, and the trigger that decides
when the item moves from roadmap to a design/plan document.

This file is component-organized; sequence numbering and design/plan authoring
conventions still follow the seq-docs convention when individual items are
picked up for implementation.

## Priority labels

Provisional priority hint: `C` (Critical), `H` (High), `M` (Medium), `L` (Low),
or `Nit`. The scheme is provisional and will be revamped; for now treat the
label as a coarse ordering hint, not a binding commitment.

## cbsd-rs (server + worker)

### Testing hardening — Phase 2 audit-remediation deferrals

- Priority: M
- Origin: implementation review at
  [./cbsd-rs/reviews/019-20260524T1031-impl-security-audit-remediation-phase-2-v1.md](./cbsd-rs/reviews/019-20260524T1031-impl-security-audit-remediation-phase-2-v1.md)
- Motivation: multiple plan-required test categories were deferred when their
  corresponding production code landed. Each guards against a specific
  regression class that today would pass code review but fail at runtime in the
  production-shaped scenario the test would model.
- Scope:
  1. **Trigger integration tests (SI-15 scenarios).** Drive
     `trigger_periodic_build` end-to-end with seeded users/tasks/scope changes
     to exercise: scope-loss after task creation, owner row hard-deleted, and
     scheduler-loop continuity (one task disables, others keep firing).
     Currently only the pure `caps_satisfy_trigger_requirements` predicate is
     unit-tested.
  2. **WebSocket over-cap protocol-level test.** Spin up a real WS server with
     the message-size caps applied, send an oversized message, assert the
     connection closes with the protocol-level error. Currently only the
     constant values are pinned in unit tests.
  3. **`tracing-test` log-capture assertions for the URI redaction policy
     (audit-rem D9).** Add `tracing-test` as a dev-dependency and assert that
     the TraceLayer span emits `path` only (no `uri`, no query). A future change
     reintroducing `uri = request.uri()` would otherwise go undetected.
  4. **`trybuild` compile-fail tests for the `SecretString` wrap (audit-rem D10
     / F13).** Assert that `#[derive(Serialize)]` over a struct holding a
     `SecretString` fails to compile, and that the inner value is unreachable
     without `.expose_secret()`. Commit 14 shipped a `tracing-test` redaction
     test plus a `static_assertions::assert_not_impl_any!(Config: Serialize)`
     guard on the real cbc `Config`; these two `trybuild` cases (the plan's
     commit-14 test items 2 and 3) are deferred because `.stderr` fixtures are
     rustc-version-brittle. Source:
     [secret-wrap review](./cbsd-rs/reviews/019-20260530T1452-impl-security-audit-remediation-secret-wrap-v1.md).
- Trigger: before the next implementation phase opens new ground that touches
  the same production code without strengthening the test guard, or sooner.

### Review periodic task capability semantics

- Priority: M
- Origin: implementation review at
  [./cbsd-rs/reviews/019-20260524T1031-impl-security-audit-remediation-phase-2-v1.md](./cbsd-rs/reviews/019-20260524T1031-impl-security-audit-remediation-phase-2-v1.md)
  (Open Question OQ1); raised again during triage of the v1 review.
- Motivation: the four periodic-task capabilities are independent, not nested.
  The current relationships are surprising and likely wrong for the intended
  deployment model:
  - `periodic:create` is required to create a new task; it is NOT implied by any
    `:manage` cap.
  - `periodic:manage:own` permits the holder to update/delete/enable/disable
    tasks they themselves created — but if they lack `periodic:create`, they
    cannot create tasks to manage, so `:own` is a dead cap in isolation.
  - `periodic:manage:any` is the admin variant of `:own` over all tasks
    regardless of `created_by`.
  - `periodic:view` is required to list/read tasks; it is NOT implied by any
    `:manage` cap, so a `:manage:any` holder cannot list the tasks they could
    mutate without also holding `:view`.

  An additional sub-question raised by review OQ1: if a task owner holds
  `:manage:own` but loses `periodic:create` or `builds:create`, should the
  scheduler trigger still fire that task? Today the trigger-time re-validation
  only checks `periodic:create` and `builds:create`, so an owner can lose
  `:manage:own` (and thereby lose their ability to disable the task via the API)
  yet retain trigger firing.

- Scope: revisit the cap mapping. Decide whether the four caps should remain
  orthogonal or whether some implicate-each-other relationships should be added.
  Decide whether `builder` (or a new `developer` role) should be granted a
  sensible default subset at seed time. Decide what the trigger-time
  re-validation should require beyond `periodic:create` and `builds:create`.
- Trigger: near term, before any production deployment grants periodic caps to
  non-admin users.

### Wrap config-time secrets (PASETO signing key, OAuth client secret)

- Priority: M
- Origin: secret-wrap implementation reviews of commit 14 (audit-rem D10 / F13);
  carried as the open question across the v1–v3 iterations.
- Motivation: commit 14 wrapped in-memory _token_ material (PASETO raw tokens,
  API and robot keys, the worker `api_key`, the cbc bearer) in
  `secrecy::SecretString`, but the process-lifetime config secrets were out of
  its scope and remain plain `String`: `SecretsConfig.token_secret_key` (the
  PASETO v4 symmetric signing key) and the Google OAuth2 client secret loaded
  from the configured secrets file. Design 019's secret contract lists "PASETO
  key bytes" as a `Secret<T>` use case, so this is unfinished audit-remediation
  surface, not a non-goal.
- Scope: wrap `token_secret_key` (and the OAuth client secret once loaded) in
  `secrecy` types; expose via `.expose_secret()` only at the encrypt/verify and
  OAuth-exchange boundaries; ensure no `Debug` or log path emits the key bytes.
- Trigger: alongside or after commit 15's tracing/`Debug` redaction sweep,
  before Phase 2 closes.

### Native TLS termination in `cbsd-server`

- Motivation: `cbsd-server` currently has no native TLS support and relies on an
  upstream TLS-terminating reverse proxy in every deployment. This is acceptable
  for current production topology but couples the security posture to operator
  discipline.
- Origin: security audit finding F6 (review
  `019-20260512T2339-impl-cbsd-rs-security-audit-v1.md`, reclassified as
  informational in the v1.1 follow-up).
- Scope: optional `axum-server` rustls integration with HSTS, an explicit
  configuration toggle, and clear documentation of when each mode is
  appropriate.
- Trigger: when operators want to deploy `cbsd-server` without an external
  reverse proxy, or when a deployment context requires TLS termination inside
  the trust boundary of the server process.

### Migrate `cbscore` from Python to Rust (`cbscore-rs` crate)

- Motivation: the worker currently invokes `cbscore` through a Python subprocess
  (`scripts/cbscore-wrapper.py`). This adds a `python3` dependency on the worker
  host, a `$PATH` resolution surface, and fork-time overhead. A native Rust
  crate consumed by `cbsd-worker` (and potentially other consumers) removes
  those concerns and tightens the type contract between the worker and the build
  engine.
- Origin: cross-cutting; subsumes security audit findings F9 (PATH resolution of
  `python3`) and F12 (dev OAuth bypass acceptable today because `cbscore`
  enforces actual upstream access to S3/Harbor/etc.).
- Scope: new `cbscore-rs` crate in the workspace, consumed in-process by
  `cbsd-worker`; the existing Python `cbscore` and the wrapper script are
  deprecated and eventually removed.
- Trigger: when the Python `cbscore` reaches feature stability for the current
  build pipeline and the team has bandwidth to port the build engine.

### Pre-commit / commit-hook tooling comparison

- Motivation: design 019 (security audit remediation) introduces a
  CI/commit-time grep gate (D10) to keep token material out of `tracing::`
  arguments. The project currently uses Lefthook for some checks. Before
  committing to a single tool for the secret-redaction gate (and any future
  policy checks of similar shape), we want a written comparison of Lefthook vs.
  `pre-commit` (the Python tool) and other alternatives, including evaluation
  criteria such as ergonomics, language ecosystem, runtime dependencies, sharing
  of hook config across contributors, CI parity, and per-file scoping.
- Origin: design 019 D10 follow-up; deferred per maintainer decision until after
  design 019 is implemented.
- Scope: research-only deliverable (short comparison document under
  `cbsd-rs/docs/`), then a follow-up decision design when the comparison is
  reviewed.
- Trigger: after design 019 implementation completes, before introducing the
  next class of commit-time policy gate.
- Related D10 follow-ups (from the
  [secret-wrap review](./cbsd-rs/reviews/019-20260530T1452-impl-security-audit-remediation-secret-wrap-v1.md)):
  - When the gate lands, the `.expose_secret()` call sites added in commit 14
    (cbsd-server, cbsd-worker, cbc) will need `// allow-expose` annotations.
  - `cbsd-worker`'s `WorkerConfig` derives `Debug` over a plain `api_key`;
    redact it as part of commit 15's tracing/`Debug` sweep.

### Converge wire-format version markers on `schema_version`

- Priority: L
- Origin: cbscore-rs design 002 (wire types & schema versioning); maintainer
  decision (2026-06-21) to keep the build report's existing `report_version`
  marker rather than rename it during the cbscore Rust port.
- Motivation: the cbscore-owned wire formats introduced by the Rust port (the
  version descriptor, release descriptor, config, and secrets file) carry a
  `schema_version` integer, but the build artifact report retains its historical
  `report_version` marker. The two names denote the same concept; renaming the
  report now would ripple into the `cbsd-worker` report parser and the
  `cbsd-server` build-report consumer for no functional gain, so they coexist
  for now.
- Scope: rename the build report's `report_version` field to `schema_version` in
  the cbscore-rs producer and update every consumer in lockstep (`cbsd-worker`'s
  report parser, `cbsd-server`'s build-report storage) so all cbs wire formats
  share one marker name and one `absent → v1` convention. Because this is an
  internal format with no cross-version interchange, it can land as a single
  coordinated commit.
- Trigger: when a change already touches the build-report consumers in
  `cbsd-worker`/`cbsd-server`, or during a broader wire-format consolidation
  pass; not urgent.

## cbc (CLI client)

(No roadmap items yet.)
