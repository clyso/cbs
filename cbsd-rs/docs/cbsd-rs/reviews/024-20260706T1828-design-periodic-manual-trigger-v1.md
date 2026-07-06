# Review: Manual Trigger for Periodic Builds (design 024)

Design reviewed:
`cbsd-rs/docs/cbsd-rs/design/024-20260706T1815-periodic-manual-trigger.md`
(commit `f6eb806`, branch `wip/cbsd-rs-periodic-retry`).

Prior authoritative designs consulted: 008 (periodic builds), 016 (role-level
scopes), 017 (robot accounts), 014 (channel/type mapping), 019-20260514T1040 and
019-20260614T2257 (security-audit-remediation, source of "audit-rem D3" and "WCP
D5"), 000-addendums.md.

Source verified against: `routes/periodic.rs`, `routes/builds.rs`,
`scheduler/trigger.rs`, `scheduler/tag_format.rs`, `db/periodic.rs`,
`db/builds.rs`, `channels/mod.rs`, `auth/extractors.rs`,
`routes/permissions.rs`, `routes/test_support.rs`, `cbc/src/periodic.rs`,
`cbc/src/builds.rs`, `cbc/src/client.rs`, `cbsd-proto/src/build.rs`,
`migrations/003_periodic_tasks.sql`, `migrations/008_periodic_manage_split.sql`,
and axum 0.8.8 / axum-core 0.5.6 `Json`/`OptionalFromRequest` source
(`~/.cargo/registry/.../axum-0.8.8/src/json.rs`).

## Executive Summary

The design is well-reasoned on the authorization model — the
requester-attribution rationale, the deliberate non-reuse of
`trigger_periodic_build`, the `can_manage_task` + `builds:create` + scope-check
stack, and the "no retry-state mutation" decision are all internally consistent
and correctly cite real code (`can_manage_task`, `PERIODIC_MANAGE_DENIED`,
`insert_build_internal`, `resolve_and_rewrite`, `interpolate_tag`,
`validate_oci_tag`, all verified to exist with the claimed signatures). However,
the design has one concrete, verifiable gap that will cause the endpoint to
behave differently from what is documented: the "absent body is equivalent to
`{}`" contract requires the `Option<Json<T>>` extractor, which is used
**nowhere** in this codebase (every other handler uses plain `Json<T>`, and the
two sibling action-endpoints `/enable`/`/disable` take no body at all). Left
unspecified, an implementer following the file's own established pattern will
ship a handler that returns 415 for a genuinely bodyless request instead of
defaulting silently — directly contradicting the design's own prose. Compounding
this, the codebase's established handler-test convention calls handlers directly
rather than through `Router::oneshot` (per `test_support.rs`'s own doc comment),
so the design's planned "omitted body" unit test will not exercise the real
HTTP-layer behavior and will not catch this defect. A second, related gap: the
priority override's promised "invalid value → 400" depends on the DTO field
being `Option<String>` with manual matching, not a direct `Option<Priority>`
enum deserialize (which — per axum's own test suite — produces 422, not 400, for
a data-classified deserialize failure). Both are resolvable with one or two
added sentences pinning the DTO shape and extractor type; the design should not
proceed to implementation without pinning them explicitly, since neither is
discoverable from the design text alone and both contradict the document's own
stated behavior.

## Critical Issues 🔴

### C1 — "Absent body" contract requires an unprecedented extractor pattern that the design does not specify, and the codebase's own test convention cannot catch a wrong choice

**Problem:** The design states (lines 94–96): "Request body (optional — absent
body, `{}`, and explicit `null` priority are all equivalent)." In axum 0.8.8,
`Json<T>` (`FromRequest`) unconditionally rejects any request that lacks a
`Content-Type: application/json` header with `MissingJsonContentType` → **415
Unsupported Media Type** — verified directly in
`axum-0.8.8/src/json.rs::impl FromRequest for Json<T>`
(`if !json_content_type(req.headers()) { return Err(MissingJsonContentType.into()) }`).
Achieving "no body ⇒ default" requires the separate `OptionalFromRequest` impl,
i.e. the handler parameter must be typed `Option<Json<TriggerTaskBody>>`, not
`Json<TriggerTaskBody>`. This pattern is used **nowhere** in `cbsd-server` today
(`grep -rn "Option<Json" cbsd-server/src` — zero hits). Every existing
periodic.rs handler with a body (`create_task`, `update_task`) takes plain
`Json<T>`; the two closest-in-spirit siblings, `enable_task`/`disable_task`,
take **no** body parameter at all (and `cbc`'s `put_empty` never sends a
`Content-Type` header — confirmed in `cbc/src/client.rs::request`, which only
calls `req.json(b)`, and thus only sets `Content-Type`, when `body` is `Some`).
An implementer extending `router()` by copying the file's own established idiom
will plausibly write `Json<TriggerTaskBody>`, which silently breaks the
documented "absent body" case with an undocumented 415 (not even present in the
design's own error table, which lists only 400/403/404/500).

Even `Option<Json<T>>` does not fully deliver "absent body ... are all
equivalent" as stated: per the same source
(`impl OptionalFromRequest for Json<T>`), if the `Content-Type` header is
present but happens to carry `application/json` on a genuinely zero-length body
(a real-world case — e.g. `curl -X POST -H "Content-Type: application/json" URL`
with no `-d`), the extractor routes into `Bytes::from_request` +
`Self::from_bytes(&[])`, which fails JSON parsing (EOF) and returns a
`JsonSyntaxError` → 400. That collateral 400 happens to land on an
already-documented status code, so it's low-risk on its own, but it means
"absent body" only cleanly defaults when **no** `Content-Type` header is sent at
all — a narrower guarantee than the design's flat "absent body ... equivalent"
phrasing suggests, and worth stating precisely rather than left to the
implementer to discover empirically.

**Impact:** A production operator (or any non-`cbc` client — Swagger UI's "Try
it out" with an empty body, a bare `curl -X POST`, a monitoring script) hitting
the trigger endpoint with a genuinely empty request gets an unexplained 415
instead of "task fires at its stored priority," which is exactly the promised,
marketed behavior and exactly the interactive "test before enabling" use case
this design exists to serve (see Problem section, bullet 1). Because `cbc`'s
`CbcClient::post` (verified: `cbc/src/client.rs::request`) _always_ calls
`.json(b)`, `cbc`'s own path never actually reaches the "no body at all" case
even when `--priority` is omitted — it always sends `{}` (or
`{"priority":null}`) with `Content-Type: application/json`. So the `cbc` CLI
itself will work correctly regardless of which extractor is chosen (as long as
`TriggerTaskBody.priority` defaults cleanly for an empty object); the blast
radius is scoped to any other caller of the raw HTTP API, plus the accuracy of
the design/OpenAPI contract itself.

**Recommendation:**

1. Explicitly state the handler signature uses `Option<Json<TriggerTaskBody>>`,
   and that a `None` value is treated identically to
   `Some(Json(TriggerTaskBody { priority: None }))`.
2. Narrow the prose claim to what is actually true: "a request with no
   `Content-Type` header, or an `application/json` body of `{}` or
   `{"priority": null}`, are all treated as 'no override.'" Explicitly drop or
   accept the edge case of an empty-body-with-json-content-type producing a
   generic 400 (JSON syntax error) rather than being folded into the "no
   override" path — that's fine, but should be a documented, deliberate choice,
   not an accident.
3. Because `test_support.rs` documents that handler tests bypass `FromRequest`
   (they call the handler function directly), the "omitted body" test case in
   the design's own Testing section will pass regardless of which extractor type
   is chosen — it proves nothing about the real HTTP behavior. Add at least one
   true HTTP-layer test (via `axum::Router` + `tower::ServiceExt::oneshot`,
   constructing a `Request` with no `Content-Type` header and an empty body)
   that exercises the actual route, or explicitly accept and document that this
   contract is unverified by the test suite.

## Significant Concerns 🟡

### S1 — Priority override validation likely returns 422, not the documented 400, depending on unstated DTO field type

**Problem:** The design states (line 104): "Any other value is rejected with 400
(strict, ...)." `cbsd_proto::Priority` (`cbsd-proto/src/build.rs`) derives
`Deserialize` with `#[serde(rename_all = "lowercase")]` and no custom visitor.
If `TriggerTaskBody.priority` is typed `Option<Priority>` — the natural choice
by analogy with `SubmitBuildBody.priority: Priority` in `routes/builds.rs`,
which is the closest sibling DTO in the same crate — then an invalid string
(e.g. `"urgent"`) fails to match any enum variant. Per axum's own test suite
(`axum-0.8.8/src/json.rs::tests::invalid_json_data`, which asserts
`StatusCode::UNPROCESSABLE_ENTITY` for exactly this class of failure —
syntactically valid JSON that fails to deserialize into the target type), this
is classified `serde_json::error::Category::Data` → `JsonDataError` → **422**,
not 400. No custom `Json`/`JsonRejection` wrapper exists anywhere in
`cbsd-server` (verified by grep) to recategorize this.

**Impact:** The design's own error table
(`| 400 | invalid priority value in body |`) would be violated by the most
natural implementation choice. Any OpenAPI-spec-driven client or contract test
asserting 400 for a bad priority value will fail against the real server.

**Recommendation:** Pin the DTO explicitly: `priority: Option<String>`, matching
the existing convention already used in the same file for
`CreateTaskBody.priority: String` and `UpdateTaskBody.priority: Option<String>`
— neither of which are typed as `cbsd_proto::Priority` today. Have the handler
manually match `"high"|"normal"|"low"` and call
`auth_error(StatusCode::BAD_REQUEST, ...)` on anything else, exactly as the
design's prose already implies. This also keeps strict-vs-lenient parsing
behavior (override strict, stored-value fallback lenient) in one place instead
of relying on two different deserialization paths for the same logical type.

### S2 — `{channel}` tag-format placeholder can interpolate to an empty string when the task relies on the requester's default channel

**Problem:** Design step 5 (interpolate tag) runs **before** step 6
(`channels::resolve_and_rewrite`, which is the only place that populates
`descriptor.channel` when the descriptor didn't specify one explicitly — see
`channels/mod.rs::resolve_and_rewrite`, step 6:
`descriptor.channel = Some(channel.name)`, executed only after channel
resolution). If the task's stored descriptor has no explicit `channel` (relying
on the resolving user's `default_channel_id`), `tag_format::resolve_placeholder`
computes `{channel}` as `descriptor.channel.clone().unwrap_or_default()` — an
**empty string** — at the point it actually runs, because `resolve_and_rewrite`
hasn't executed yet. This is not new to this design: `scheduler::trigger.rs` has
the identical step ordering (interpolate at step 5, `resolve_and_rewrite` at
step 8), so this is a latent behavior inherited from design 008, not introduced
by 024.

**Impact:** For any task whose descriptor omits an explicit `channel` (relying
on the default), a `tag_format` containing `{channel}` silently produces a wrong
(empty) segment in the interpolated tag — for **both** the pre-existing
scheduled path and this design's new manual path. This is exactly the failure
mode design 024 exists to catch early ("to test a task's descriptor and tag
format ... without waiting for the next cron fire" — Problem, bullet 1): an
operator manually firing a task to validate its tag format would see this bug
immediately, and correctly attribute it to the trigger feature, even though the
root cause is pre-existing and shared with the scheduled path.

**Recommendation:** Not necessarily a blocker for 024 specifically (it's
inherited, shared behavior), but the design should not silently carry this
forward without comment, especially given the design explicitly walks through
the tag-interpolation step order in detail. Either (a) note this as a known
pre-existing limitation inherited from 008 and file a follow-up design addendum
against 008/`tag_format.rs` to resolve the channel before interpolation for both
trigger paths, or (b) reorder locally for the manual path (resolve channel/type
first, then interpolate) if the two steps can be safely swapped without behavior
regressions elsewhere — worth investigating since this design already touches
the exact step sequence.

### S3 — Design's scheduled-vs-manual comparison table overstates the scheduled path's scope re-validation relative to audit-rem D3's normative text

**Problem:** Design 024's comparison table (line 42) states "Scopes checked |
owner's | requester's" for the scheduled vs. manual paths, framing this as
parity — both paths "check scopes," just for different identities. But
`audit-rem` D3 (019-20260514T1040, lines 392–404) is explicit and normative: the
scheduler trigger "MUST re-validate the full stored descriptor (**channel scope,
repository scope, every component scope**) against the task owner's current
effective scopes." The actual `scheduler/trigger.rs` code only calls
`resolve_and_rewrite` (channel/type scope) — there is no call to
`require_scopes_all` or any repository-scope check anywhere in
`scheduler/trigger.rs` or `channels/mod.rs` (verified by grep: zero hits for
`require_scopes_all` or `ScopeType::Repository` outside `routes/builds.rs` and
`routes/periodic.rs`'s creation/update paths). This means the scheduled path
does **not** currently re-validate repository scope at every fire — only at task
creation/update time — a gap relative to D3's own "MUST" language that appears
to have shipped short of the documented requirement (the implementation plan's
commit 12 description, `docs/cbsd-rs/plans/019-20260516T1033-...`, describes
re-validating "current effective capabilities," which is narrower than D3's
"full stored descriptor" language).

By contrast, design 024's own manual-trigger step 3 **does** add a fresh
`require_scopes_all` repository-scope check against the requester — i.e. 024's
new endpoint is actually more faithful to D3's spirit than the existing
scheduled path it's being compared against.

**Impact:** This is not a defect introduced by 024 and is out of scope for 024
to fix. But 024's own narrative uses this comparison table as its central
justification for not reusing `trigger_periodic_build`, and an engineer or
auditor reading the table at face value will conclude the scheduled path already
re-validates repository scope per D3 — which is not true today. Leaving this
uncorrected risks the inaccurate picture propagating (it would be a natural
thing to cite in a future audit as "already fixed").

**Recommendation:** Either soften the table's "owner's" cell to specifically
note "channel/type scope only — repository scope re-checked at creation/update
time, not at each fire (audit-rem D3 gap, tracked separately)," or file the gap
explicitly against 008/audit-rem-D3 via a `000-addendums.md` entry so it's
visible independent of this review.

## Minor Observations 🟢

- **"Mirrors `POST /api/builds`" is an overstatement.** The actual
  `SubmitBuildResponse` (`routes/builds.rs`) has fields `id`, `state`,
  `is_robot`, `warning` — materially different from the design's `build_id`,
  `state`, `tag`, `priority`, `warning` (renamed `id`, two new fields, one
  dropped field). The design's rationale for the rename (`build_id` to
  disambiguate from the path's task id) is sound, but "mirrors" undersells how
  different the two shapes actually are; suggest "modeled on" instead, and
  consider whether `is_robot` should be carried over for the same UI-rendering
  reason `submit_build` added it.
- **`updated_at` semantics diverge between trigger paths.** The scheduler's
  `update_trigger_success` (`db/periodic.rs`) bumps `updated_at` on every
  successful scheduled fire; the design's new `record_manual_trigger` SQL does
  not touch `updated_at` at all. Neither choice is wrong, but the inconsistency
  (what does "last updated" mean for a periodic task — "last time its definition
  changed" or "last time it fired by any means"?) isn't explained, and now
  differs by which path most recently touched the row. Worth a one-line
  rationale.
- **OpenAPI optional-body accuracy unverified.** The design says "Standard
  utoipa integration ... Spec collection is automatic," but doesn't address
  whether `#[utoipa::path(request_body = TriggerTaskBody, ...)]`'s shorthand
  form marks the body `required: true` in the generated spec by default
  (utoipa's common behavior for named-type `request_body` shorthand). If so, the
  generated OpenAPI document would misstate the body as required even though
  `Option<Json<T>>` makes it optional at the handler level. Verify at
  implementation time and use utoipa's explicit
  `request_body(content = ..., ...)` form with the correct optionality if the
  shorthand defaults wrong.
- **Response naming**: `tag` and `priority` in the 202 response are new,
  reasonable additions given the "test before enabling" use case (an operator
  wants to see the interpolated tag immediately) — no issue, just noting they
  have no precedent in `SubmitBuildResponse` to check against, so utoipa schema
  naming/casing should be double-checked at review time for the actual PR.

## Strengths

- **Correct, verified authorization stack.** `can_manage_task`,
  `PERIODIC_MANAGE_DENIED`, `has_cap`, `require_scopes_all`,
  `ROBOT_FORBIDDEN_CAPS`, and `resolve_and_rewrite` are all cited with
  signatures and semantics that match the actual source exactly. The design
  correctly identifies that `periodic:manage:{own,any}` + `builds:create` (with
  scopes) is the right gate, and correctly identifies that `periodic:create`
  must **not** be required (a subtle and easy-to-get-wrong distinction between
  "create a new task" and "operate an existing one").
- **Requester-attribution rationale is sound and well-argued.** The design
  correctly distinguishes itself from 008's "permission bypass" security note
  (which applies only to unattended scheduler fires) and gives a concrete,
  credible operational benefit (an admin can safely fire a task whose owner has
  been deactivated).
- **Deliberate non-mutation of retry state is correctly reasoned.** The argument
  that a successful manual fire doesn't prove the _scheduled_ path is healthy
  (different identity, different scope set) is correct and avoids a
  plausible-looking but wrong shortcut (auto-clearing backoff on any successful
  build from the task).
- **Concurrency analysis is accurate.** The claims about FK behavior
  (`ON DELETE SET NULL`), the queue mutex already serializing
  `insert_build_internal`, and last-writer-wins being acceptable for the two
  purely-visibility bookkeeping columns are all verified correct against the
  actual schema and code.
- **HTTP verb choice (POST vs. the siblings' PUT) is correctly justified** by
  non-idempotency, not just copied by convention.
- **No new capability strings, no schema migration** — the design correctly
  identifies that everything needed (columns, `KNOWN_CAPS` entries) already
  exists, minimizing blast radius.

## Open Questions

1. Should the handler validate that the requester's user row still has
   `active = true`, or is relying on the `AuthUser` extractor's implicit
   deactivation check (`load_authed_user`, which already 401s deactivated users
   before the handler runs) sufficient? (It is sufficient — verified in
   `auth/extractors.rs` — but the design doesn't say so explicitly, and a reader
   unfamiliar with the auth layer might wonder why the scheduled path's "check
   owner active" step 1 has no analogue here.)
2. What should the OpenAPI spec say about the optional request body's `required`
   flag, and does the utoipa version in use (`utoipa`/`utoipa-axum`, versions
   not pinned in this design) need any special handling to mark it correctly?
3. Is the `{channel}`-before-`resolve_and_rewrite` ordering (S2) something this
   design should fix now that it's touching the exact same step sequence for a
   second call site, or strictly out of scope pending a dedicated 008 addendum?

## Confidence Scoring

| Item                                                                                              | Points | Description                                                                                                                                                                                  |
| ------------------------------------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                                    | 100    |                                                                                                                                                                                              |
| C1 (part a) — "absent body" contract needs unprecedented `Option<Json<T>>`, unspecified in design | −5     | D8: Spec deviation — axum's actual `Json`/`Option<Json>` semantics contradict the stated "absent body ... equivalent" behavior unless explicitly pinned                                      |
| C1 (part b) — codebase's function-level test convention cannot verify the HTTP-layer contract     | −15    | D5: Untested critical path — the design's own planned "omitted body" test is verified (via `test_support.rs`) to bypass `FromRequest` entirely, so a wrong extractor choice ships undetected |
| S1 — priority override likely 422 not 400 depending on DTO field type                             | −5     | D8: Spec deviation — axum's own test suite confirms enum-deserialize failures are classified as 422 (`JsonDataError`), contradicting the design's documented 400                             |
| S2 — `{channel}` placeholder can interpolate empty when relying on default channel                | −5     | D8: Spec deviation from 008's tag-format table, carried into the new endpoint without comment                                                                                                |
| S3 — comparison table overstates scheduled-path scope re-validation vs. audit-rem D3              | −5     | D11: Missing documentation — table's "owner's" scopes cell doesn't reflect that repository-scope re-validation is absent from `scheduler/trigger.rs` today                                   |
| Minor — "mirrors POST /api/builds" overstated                                                     | −5     | D11: Missing documentation — response shape materially differs from the endpoint it's claimed to mirror                                                                                      |
| Minor — `updated_at` semantics diverge between trigger paths, unexplained                         | −5     | D11: Missing documentation                                                                                                                                                                   |
| Minor — OpenAPI required-flag on optional body unverified                                         | −5     | D11: Missing documentation — needs verification against the utoipa version in use                                                                                                            |
| **Total**                                                                                         | **55** |                                                                                                                                                                                              |

Per the interpretation scale (50–74: "Significant issues. Must address before
proceeding"), this design needs the two Critical-issue items (C1) resolved with
explicit text before implementation starts, and the Significant items (S1–S3)
addressed or explicitly accepted in the design text. Nothing found rises to
"fundamentally flawed" — the authorization model, data model, and concurrency
reasoning are all sound and correctly grounded in the actual code.

## Findings Ordered by Severity

1. 🔴 **C1** — "Absent body" semantics require the codebase's first-ever use of
   `Option<Json<T>>`; unspecified in the design, likely to be implemented as
   plain `Json<T>` (415 on bodyless requests) by analogy with every sibling
   handler, and the design's own planned test cannot detect the mistake because
   handler tests in this codebase bypass axum's request-extraction layer.
2. 🟡 **S1** — Priority override's promised "400 on invalid value" likely ships
   as 422 unless the DTO is explicitly pinned to `Option<String>` + manual match
   rather than a direct `Option<Priority>` enum deserialize.
3. 🟡 **S2** — `{channel}` tag-format placeholder can interpolate to an empty
   string when the task's descriptor relies on the resolving user's default
   channel, because interpolation runs before channel resolution in both the
   scheduled (008) and new manual step ordering; inherited, not introduced, but
   newly load-bearing given this endpoint's stated "test the tag format"
   purpose.
4. 🟡 **S3** — The scheduled-vs-manual comparison table implies the scheduled
   path re-validates repository scope at every fire per audit-rem D3's "MUST"
   language; the actual `scheduler/trigger.rs` only re-validates channel/type
   scope, not repository scope — a pre-existing gap the design's narrative
   should not silently reinforce as closed.
5. 🟢 Minor — "mirrors `POST /api/builds`" response-shape claim is materially
   imprecise (renamed/added/dropped fields).
6. 🟢 Minor — `updated_at` bookkeeping semantics diverge unexplained between the
   scheduled and manual trigger paths.
7. 🟢 Minor — OpenAPI `required` flag on the optional request body needs
   verification against the utoipa version/macro form in use.

## Verdict

**Revise and re-review.** The authorization model, data model, and concurrency
story are sound and ready to build on, but the design cannot proceed to
implementation as written: the optional-body contract (C1) is a concrete,
verifiable correctness gap that contradicts the document's own stated behavior,
is invisible to the codebase's normal test methodology, and directly undercuts
the endpoint's primary "quick, frictionless manual test" use case if a caller
sends a genuinely bodyless request. Add one paragraph pinning the extractor type
(`Option<Json<TriggerTaskBody>>`) and the DTO field shape
(`priority: Option<String>` with manual matching), address or explicitly accept
S2/S3, and this is ready to implement.
