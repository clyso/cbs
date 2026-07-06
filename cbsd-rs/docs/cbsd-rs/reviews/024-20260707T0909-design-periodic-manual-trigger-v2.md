# Review: Manual Trigger for Periodic Builds (design 024) — v2

Design reviewed:
`cbsd-rs/docs/cbsd-rs/design/024-20260706T1815-periodic-manual-trigger.md`
(commit `867495a2`, branch `wip/cbsd-rs-periodic-retry`, revised in response to
`cbsd-rs/docs/cbsd-rs/reviews/024-20260706T1828-design-periodic-manual-trigger-v1.md`).

This is a re-review. Every v1 finding is re-verified against the current design
text and, where the design makes a new factual/technical claim, against the
actual source (axum 0.8.8/axum-core 0.5.6, utoipa-gen 5.4.0,
`cbsd-server/src/app.rs`, `cbsd-server/src/routes/periodic.rs`,
`cbsd-server/src/routes/test_support.rs`, `cbsd-server/src/auth/paseto.rs`,
`cbsd-server/Cargo.toml`). No claim from the coordinator's changelist or the
design's own prose was taken on trust without independent verification.

## Executive Summary

All four v1 findings that mattered (C1, S1, S2, S3) are resolved correctly and
verifiably. C1 in particular is not just patched but grounded in real, existing
prior art: `build_router(state, session_layer)` + `tower::ServiceExt::oneshot`
is already used by `app.rs`'s own tests (`rest_body_over_limit_returns_413`), so
the design's mandate to use the same pattern for the optional-body HTTP-layer
tests is concretely executable, not aspirational. S1's `Option<String>` + manual
match, S2's documented tag/channel-ordering trade-off, and S3's corrected
scope-checking table are all accurate and well-reasoned. However, one of the
"resolved" minors was not actually fixed: the OpenAPI section's new guidance to
switch from the shorthand `request_body = TriggerTaskBody` to the parenthesized
`request_body(content = TriggerTaskBody, description = "...")` form does **not**
change whether the generated spec marks the body `required` — verified directly
against `utoipa-gen-5.4.0`'s `RequestBodyAttr::to_tokens`, `required` is
computed purely from whether the **content type itself** is `Option<...>`
(`!t.is_option()`), independent of which macro syntax form is used. The design's
own code sample omits the `Option<...>` wrapper, so as written it will still
emit `requestBody.required: true` — the exact defect it claims to fix. This is a
narrow, self-contained, doc/spec-accuracy issue (it does not affect runtime
request handling, which is correct), and the design already mandates a
verification step that would catch it — but the code sample given is factually
wrong and needs a one-line correction before implementation.

## Re-verification of v1 findings

### C1 — "Absent body" extractor pattern (was Critical) → **Resolved, correctly**

The design now states (lines 115–127) that the handler parameter is
`Option<Json<TriggerTaskBody>>`, explicitly calls this "load-bearing, not
stylistic," and gives the precise rationale (plain `Json<T>` 415s on a request
with no `Content-Type` header). This matches axum 0.8.8's actual
`impl OptionalFromRequest for Json<T>` exactly: header absent → `Ok(None)`;
header present and not JSON → `Err(MissingJsonContentType)` (415); header
present and JSON with a zero-length body → `JsonSyntaxError` (400, EOF category)
— all three cases are now correctly reflected in the error table (lines 229–243)
and in the new mandatory HTTP-layer test list (lines 363–375).

The mandatory-HTTP-layer-tests requirement is not just correct in principle —
it's grounded in real precedent. `cbsd-server/src/app.rs` already has exactly
this pattern (`build_router(state, session_layer)` +
`tower::ServiceExt::oneshot(req)`, `rest_body_over_limit_returns_413`, lines
210–231), and `test_support.rs` already exposes `test_session_layer` for
constructing it. The four listed cases (no body/no header → 202; `{}` → 202;
`{"priority": "urgent"}` → 400; `text/plain` → 415) map 1:1 onto the
`Json`/`OptionalFromRequest` branches verified in v1's review. This resolves C1
completely — both the missing-extractor-type gap and the
untestable-by-direct-call-convention gap.

### S1 — Priority DTO type (was Significant) → **Resolved, correctly**

`priority: Option<String>` with manual matching (lines 135–141) is exactly the
fix recommended in v1, with the correct rationale (`JsonDataError` → 422 for an
enum-variant mismatch, contradicting the documented 400). It also correctly
distinguishes what still legitimately produces 422 under the new DTO shape — a
non-string JSON value (e.g. `{"priority": 123}`) still fails to deserialize into
`Option<String>` and correctly lands at 422 per the new error table row, while a
syntactically-valid-but-semantically-wrong string (`"urgent"`) deserializes fine
and is correctly rejected by the handler's own manual match with 400. This is an
accurate and complete resolution.

### S2 — `{channel}` tag placeholder ordering (was Significant) → **Resolved, appropriately scoped**

The design now explicitly documents the inherited ordering issue (lines
161–168), correctly attributes it to the same step order used by
`scheduler/trigger.rs`, and gives a materially better rationale than v1's review
suggested: keeping the manual and scheduled paths' interpolation order identical
means a manual trigger produces the **same** tag a scheduled fire would for the
same task — which matters precisely because this endpoint's stated purpose is
"test the tag format before enabling," and a manual-path-only fix would make
manual test-fires diverge from what the schedule actually produces, undermining
that exact use case. Deferring the real fix to 008/`tag_format.rs` scope is
reasonable. This is a better outcome than the fix I suggested in v1; no further
action needed on this design.

### S3 — Scheduled-path scope re-validation table (was Significant) → **Resolved, correctly**

The "Scopes checked" row (line 42) and the added note paragraph (lines 49–56)
now accurately state that the scheduled path only re-validates channel/type
scope at fire time, not repository scope, name this as a pre-existing gap
relative to audit-rem D3's "full stored descriptor" language, and correctly note
the manual path has no equivalent gap. This matches what was independently
verified in v1 by grepping `scheduler/trigger.rs` and `channels/mod.rs` for
`require_scopes_all` (zero hits outside `routes/builds.rs` and
`routes/periodic.rs`'s creation/update paths). Fully resolved.

### Minors (was three items) → **Two resolved correctly, one resolved incorrectly (see new finding below)**

- "mirrors" → "modeled on" with an explicit field diff (lines 201–204) and
  `is_robot` added back to the response (lines 212, 223–224) with the same
  rationale `SubmitBuildResponse` uses it for — resolved correctly.
- `updated_at` non-touch rationale (lines 276–281) explicitly names the
  scheduler's existing conflation of "definition changed" and "last fired,"
  states this design does not replicate it, and correctly scopes a cleanup of
  the scheduler's own behavior as 008 follow-up material, not this design's job
  — resolved correctly.
- OpenAPI `requestBody.required` — **not actually fixed**; see New Finding
  below.

The v1 open question about requester-active checking is also now answered
explicitly (lines 89–91), correctly citing that `AuthUser`'s extraction path
already rejects deactivated users before any handler runs (matches
`auth/extractors.rs::load_authed_user`, re-verified).

## New Finding (introduced by the v2 revision)

### N1 — OpenAPI `request_body` fix does not achieve its stated goal 🟡

**Problem:** The design's OpenAPI section (lines 310–317) says: "the shorthand
`request_body = TriggerTaskBody` must not be used if it marks the body required
... Use the explicit form and verify the emitted `requestBody.required` is
`false`," and gives this code sample:

```rust
request_body(content = TriggerTaskBody, description = "...")
```

This is not correct. Verified directly against `utoipa-gen` 5.4.0's
`RequestBodyAttr`
(`~/.cargo/registry/.../utoipa-gen-5.4.0/src/path/request_body.rs`): the
`content` field is populated identically whether the shorthand
(`request_body = X`) or parenthesized (`request_body(content = X, ...)`) form is
used — both funnel into the same `MediaTypeAttr::parse_schema` call. The
`required` flag is computed in `ToTokensDiagnostics::to_tokens` purely from
whether the parsed content **type** is an `Option<...>`:

```rust
any_required = any_required
    || media_type.schema.get_type_tree()?.as_ref()
        .map(|t| !t.is_option())
        .unwrap_or(false);

if any_required {
    tokens.extend(quote! { .required(Some(#required)) })
}
```

The macro's own doc comment states this explicitly: "To define optional request
body just wrap the type in `Option<type>`," with the canonical example
`request_body = Option<[Foo]>`. Switching syntax **forms** (shorthand →
parenthesized) without wrapping the content type in `Option<...>` changes
nothing about the `required` computation. Since the design's code sample uses
`content = TriggerTaskBody` (not `content = Option<TriggerTaskBody>`), the
generated spec will still emit `required: Some(true)` — the exact defect the
section claims to resolve.

This codebase's own precedent confirms the annotation is load-bearing and not
auto-inferred from the handler signature: `create_task`/`update_task` in
`routes/periodic.rs` both use explicit `request_body = CreateTaskBody` /
`request_body = UpdateTaskBody` annotations (and `utoipa-server/Cargo.toml` does
not enable the `axum_extras` feature that would otherwise let `utoipa-axum`
infer request bodies from handler parameter types), so this is not a case where
the runtime `Option<Json<T>>` handler signature will be picked up automatically
to correct the spec.

**Impact:** Narrow — this affects only the generated OpenAPI document's
metadata, not runtime request handling (the handler's actual
`Option<Json<TriggerTaskBody>>` signature is correct and independent of this
annotation). Any OpenAPI-driven codegen or contract test that trusts
`requestBody.required: true` would incorrectly force clients to always send a
body for this endpoint, or a documentation reviewer manually checking
`requestBody.required == false` (as the design's own text instructs) would
immediately catch this — the design already mandates the exact check that
exposes its own error, which is a good safety net, but the code sample as
written will fail that self-mandated check.

**Recommendation:** Correct the code sample to wrap the content type in
`Option<...>`, consistent with the runtime handler's own type:

```rust
request_body(content = Option<TriggerTaskBody>, description = "...")
```

(or equivalently, the simpler shorthand
`request_body = Option<TriggerTaskBody>,` — per the same source, the
parenthesized form is only needed here because a `description` is also being
attached, not because of the optionality itself).

## Minor Observations 🟢

- The new error table's "400 | zero-length body sent with
  `Content-Type: application/json`" row is technically accurate but narrower
  than the full set of cases that land on the same code path (any
  malformed-but-non-empty JSON body with a JSON content type also produces 400
  via the same `JsonSyntaxError` branch). Not worth a table row of its own, but
  a parenthetical ("or any invalid JSON syntax") would remove any ambiguity for
  an implementer reading the table literally.
- The mandatory HTTP-layer tests (lines 371–375) correctly specify the requests
  to send but don't mention that they'll need an authenticated request (a real
  PASETO bearer token via `auth::paseto::token_create` plus a seeded user/role,
  since every periodic endpoint requires `AuthUser`). This is
  implementation-plan-level detail rather than design-level, so not a finding —
  just worth flagging so the eventual plan document doesn't under-scope this
  test's setup cost.

## Strengths (carried over and reinforced)

- The C1 fix is unusually well-grounded: rather than asserting the
  `Option<Json<T>>` pattern is safe, the design ties the required HTTP-layer
  test technique directly to an already-existing, already-passing test in
  `app.rs`, which is the strongest possible form of "this is testable and here's
  the proof it's already been done once."
- S2's resolution (keep scheduled/manual tag-interpolation order identical, even
  though a per-path fix was available) reflects a good instinct: local
  correctness (fixing `{channel}` just for the manual path) would have created a
  worse global property (scheduled and manual fires of the same task producing
  different tags), which would have undermined the endpoint's own "test before
  enabling" premise. This is exactly the kind of trade-off reasoning a design
  document should make explicit, and it does.
- S3's note paragraph turns what could have been left as an inaccurate,
  confidence-inflating table cell into an honest, precisely-scoped admission of
  a pre-existing gap — without taking on the (out-of-scope) work of fixing it in
  this design.

## Open Questions

None remaining that block implementation, conditional on N1's one-line fix.

## Confidence Scoring

| Item                                                                                                   | Points | Description                                                                                                                  |
| ------------------------------------------------------------------------------------------------------ | ------ | ---------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                                         | 100    |                                                                                                                              |
| N1 — OpenAPI `Option<...>` wrapper omitted from the `request_body` fix, `required: true` still emitted | −5     | D8: Spec deviation — code sample contradicts utoipa-gen 5.4.0's actual `required`-computation logic, verified against source |
| **Total**                                                                                              | **95** |                                                                                                                              |

All four substantive v1 findings (C1, S1, S2, S3) are fully and correctly
resolved with source-verified, well-reasoned changes; two of three v1 minors are
also fully resolved. The sole surviving issue (N1) is narrow, isolated to
OpenAPI spec metadata, does not affect runtime correctness, and is a one-line
fix. Per the interpretation scale (90–100: "Ready to merge. Minor or no
issues"), this design is ready to implement once N1 is corrected.

## Findings Ordered by Severity

1. 🟡 **N1** (new in v2) — OpenAPI `request_body` fix as written does not mark
   the body optional in the generated spec; the code sample must wrap the
   content type in `Option<...>`
   (`request_body(content = Option<TriggerTaskBody>, ...)`), not merely switch
   from the shorthand to the parenthesized macro form.
2. 🟢 Minor — "zero-length body" error-table row could note it also covers any
   malformed (non-empty) JSON body with a JSON content type.
3. 🟢 Minor — mandatory HTTP-layer tests will need authenticated requests (real
   PASETO token + seeded user/role); worth flagging for the implementation plan,
   not a design defect.

## Verdict

**Go, with one condition.** All Critical and Significant findings from v1 are
resolved correctly and verifiably, several with reasoning better than what v1
itself proposed. The only remaining issue (N1) is self-contained, affects
OpenAPI documentation accuracy only (not runtime behavior), and is a one-line
correction: change `request_body(content = TriggerTaskBody, ...)` to
`request_body(content = Option<TriggerTaskBody>, ...)` (or the equivalent
`request_body = Option<TriggerTaskBody>` shorthand). No further design review
round is required — fix N1 during implementation and confirm via the design's
own already-mandated `requestBody.required == false` check.
