# 017 — Robot Accounts: Implementation Plan Review v1

**Plan:** `docs/cbsd-rs/plans/017-20260419T2123-robot-accounts.md`\
**Design:** `docs/cbsd-rs/design/017-20260417T1130-robot-accounts.md` (v3)\
**Prior reviews:**
`docs/cbsd-rs/reviews/017-20260417T1841-design-robot-accounts-v1.md`,\
`docs/cbsd-rs/reviews/017-20260419T0905-design-robot-accounts-v2.md`

---

## 1. Summary Assessment

The plan is structurally sound and demonstrates careful attention to dependency
ordering, commit granularity, and codebase accuracy. The preparatory-phase
rationale is well argued, migration numbering is correct, and every codebase
claim checked against source was accurate. However, one blocker — with two
manifestations — stands between this plan and implementation: the plan conflates
`disable` (reversible) and `tombstone` (permanent) semantics for robot targets.
The `deactivate_entity` handler revokes tokens where the design mandates leaving
them intact, and the `activate_entity` handler returns 400 where the design
mandates restoring function. Both deviations share the same root cause and must
be corrected together before implementation begins.

---

## 2. Strengths

**Dependency ordering is correct and well-justified.** The graph from commit 3 →
4 → 5 → 6 → 7 → 8 is technically sound. Commit 3's rationale — consolidate the
last-admin guard before adding the `is_robot = 0` filter so that filter takes
effect at every call site automatically — is exactly the right reason to split
it from commit 4.

**Codebase claims are accurate.** Every verifiable claim in the plan was checked
against the live source:

- Inline `SELECT COUNT` in `routes/admin.rs` at lines 98–111: confirmed.
- `ApiKeyCache` / `CachedApiKey` struct layout in `auth/api_keys.rs`: confirmed;
  rename to `TokenCache` / `CachedToken` is a real and necessary change.
- `AppState.api_key_cache: Arc<Mutex<ApiKeyCache>>` in `app.rs`: confirmed.
- `count_active_wildcard_holders` pool-level signature (no `_tx` overload yet)
  in `db/roles.rs`: confirmed; plan's commit 3 correctly anticipates adding one.
- `DeleteArgs { force: bool }` in `cbc/src/admin/roles.rs`: confirmed; no
  `--yes-i-really-mean-it` today.
- `DeregisterArgs { id: String }` and `PeriodicDeleteArgs { id: String }` both
  lack a confirmation flag: confirmed.
- Migration numbering (005 is the current last file; plan adds 006 and 007):
  confirmed.
- `DeleteRoleQuery { force: bool }` and `?force` query parameter in
  `routes/permissions.rs`: confirmed — important because commit 2 removes this
  server-side.

**Commit 6 size justification is convincing.** The plan correctly identifies
that splitting robot provisioning from robot authentication would produce a
broken intermediate commit (tokens issued but not verifiable), and cites the
commit-granularity principle. The ~900–1100 LOC estimate is appropriate for the
scope.

**Forbidden-cap strip placement is architecturally correct.** Stripping at every
auth verification (cache hit and miss) rather than at assignment time or at
token creation time means a role update that adds a forbidden cap takes effect
at the next request, with no stale cache window. The design's intent is
preserved.

**Partial unique index semantics are correctly understood.** The plan's
treatment of
`idx_robot_tokens_active ON robot_tokens(robot_email) WHERE revoked = 0` as the
mechanical serialisation gate for `create_or_revive` is correct — the partial
index enforces at most one non-revoked token per robot at the DB level
regardless of application logic.

**P1 exclusion rationale is defensible.** Excluding `admin users deactivate`
from the `--yes-i-really-mean-it` flag is explicitly justified (reversible via
activate) and aligns with the plan's stated scope for irreversible-only
coverage.

---

## 3. Blockers

### B1 — Plan conflates disable and tombstone semantics for robot targets (two manifestations)

The plan's treatment of robot disable/tombstone contradicts the approved v3
design in two places. Both stem from the same root cause: the plan treats
`deactivate_entity` as equivalent to `tombstone_robot` when they are
semantically distinct operations.

**Design v3, disable/tombstone comparison table:**

| Operation | `users.active` | `robot_tokens` | Reversible? |
| --------- | -------------- | -------------- | ----------- |
| disable   | 0              | Unchanged      | Yes         |
| tombstone | 0              | All revoked    | Via create  |

**Manifestation 1: `deactivate_entity` revokes tokens**

Commit 6 specifies that the `deactivate_entity` handler, when
`target.is_robot = 1`, calls `revoke_all_tokens_for_robot` (plan, line 307–308).
The dependent test asserts `robot_tokens` rows have `revoked = 1` after entity
deactivate (plan, line 366–367).

Design v3, lines 1006–1009:

> Update the entity `deactivate` handler to branch on `is_robot`: human →
> existing behaviour (revoke PASETO tokens + API keys + purge caches); robot →
> set `active = 0` and purge the robot's cache entries only, **leaving
> `robot_tokens` rows untouched (disable semantics, reversible).**

Design v3, lines 1053–1054 (test checklist):

> Entity deactivate for a robot target **leaves `robot_tokens` rows intact** and
> can be reversed by `activate`.

**Manifestation 2: `activate_entity` returns 400 for all robot rows**

Commit 6 specifies (plan, line 368–371):

> Entity `activate` on a tombstoned robot: NOT exposed yet — that path is
> covered by `POST /api/admin/robots` (revive-via-create). **The entity
> `activate` handler for an `is_robot = 1` row should return 400** pointing at
> the create endpoint.

Returning 400 for _any_ `is_robot = 1` row destroys the reversibility guarantee
even if manifestation 1 is fixed. A robot disabled by `deactivate_entity`
(tokens intact, `active = 0`) cannot be re-enabled by `activate_entity` — the
400 response leaves no escape path other than full revive-via-create, which
issues new credentials. The "reversible" property in the design's table becomes
a dead letter.

**Why it matters:** Together, the two manifestations mean the plan has no
working disable/enable cycle for robots. An admin who disables a robot to
investigate a permissions issue cannot restore it to the prior state. The design
explicitly distinguishes disable (reversible, tokens intact) from tombstone
(permanent, tokens revoked) for this operational reason.

**Resolution:** In commit 6:

1. `deactivate_entity` for robot targets: set `users.active = 0` + purge cache
   by owner. Do _not_ call `revoke_all_tokens_for_robot`.
2. `activate_entity` for robot targets: when `is_robot = 1` and the row has
   `active = 0`, set `users.active = 1` (same as human activate). Return 400
   _only_ when the row has `active = 0` and _also_ has no non-revoked
   `robot_tokens` rows (i.e., the robot was tombstoned, not merely disabled) —
   that is the condition where revive-via-create is the correct path.
3. Update the `deactivate_entity` test: assert `robot_tokens` rows have
   `revoked = 0` after deactivate.
4. Add a test for the full disable/enable cycle: deactivate → auth fails →
   activate → auth succeeds (token unchanged).

---

## 4. Major Concerns

### M1 — Commit 2 removes the server-side `?force` parameter: silent API contract break not surfaced in commit message

**What:** The plan states (commit 2 files table, `permissions.rs` row): "remove
`force` query parameter if present; behaviour is the same as `--force` was
(always cascade)." This is framed in the narrative as "purely client-side UX."
It is not — the `?force` query parameter is part of the server's external API
contract, accepted today in `DELETE /api/permissions/roles/{name}`. Removing it
is a breaking change for any caller that currently passes `?force=true`
(including the existing `cbc` binary before commit 2 lands, and any external
tooling).

**Why it matters:** The plan says the commit message should "flag this as a
breaking API change for external consumers (none known)" — but that notice
appears in commit 4's spec, not commit 2's. Commit 2 makes the breaking change
silently. If commits are applied incrementally (e.g., cherry-picked to a release
branch), the break lands without the warning.

**Resolution:** Add an explicit note in commit 2's specification that the
`?force` parameter removal is a breaking server API change, and that the commit
message must call it out. This is a documentation fix to the plan, not a design
change. Alternatively, consider keeping the `?force` parameter on the server
(silently ignoring it) for one commit cycle and removing it only in commit 4
under the endpoint reshape, which makes the break more visible.

### M2 — Missing test: forbidden-cap strip after role update post-assignment

**What:** The plan tests that a role containing a forbidden cap is rejected at
robot creation (commit 6 test: "Create with a role containing a forbidden cap:
400"). It does not test the complementary scenario: a robot is created with a
clean role, that role is later updated (via
`PUT /api/admin/entity/{email}/roles`) to include a forbidden cap, and the
robot's next auth request does not carry the forbidden cap.

**Why it matters:** The forbidden-cap strip is described as operating at every
auth verification event, specifically to catch post-assignment role mutations.
If only the creation-time rejection is tested, the runtime strip is exercised
only by code inspection. A cache-hit scenario where the cached `CachedToken` was
created before the role mutation and a cache-miss scenario where the roles are
re-loaded from DB, both stripping correctly, are each distinct paths.

**Resolution:** Add a test to commit 6's test suite (or commit 7 if role-update
interactions fall naturally there): assign a robot a role, update the role via
the entity roles endpoint to add `robots:manage`, authenticate — assert the
capability is absent. This does not require a real cache eviction; a fresh
extractor call on a new connection exercises the DB-load path.

---

## 5. Minor Issues

- **Commit 5 adds usage-tracking columns to `tokens` and `api_keys` with no
  reader.** `first_used_at` and `last_used_at` are written but no `GET` endpoint
  or `list` response exposes them in this commit. This is not a blocker — the
  plan explicitly positions commit 5 as symmetric groundwork for commit 6, which
  adds the robot equivalents and a response surface — but it means commit 5
  contains write-only columns until commit 6 lands. The plan should note this
  explicitly so a reviewer does not flag the columns as dead-write at commit 5's
  review time.

- **`create_or_revive` 409 branches need explicit test cases for both paths.**
  The plan specifies "Concurrent create of same tombstoned name: one wins with
  `revived: true`, the other returns 409." This covers the concurrent-revive
  race. It does not explicitly cover the non-concurrent 409 paths: (a)
  `POST /api/admin/robots` when the row is already active (not a revive), and
  (b) `POST /api/admin/robots` with a name whose row is a human (not a robot).
  Both are distinct code branches that should be in the test matrix.

- **Commit 4's line-count estimate (~500–700) may be optimistic.** The endpoint
  migration table moves eight routes, updates `cbc` URL literals, and
  restructures two router files. Path-shuffle PRs routinely exceed estimates.
  This is not a blocker but the author should be prepared for 700–900 and should
  not split the commit mid-migration if it runs long.

- **`mark_used` invoked outside request-path transaction (plan, commit 5)** —
  the plan correctly states "a failed usage-update must not fail the request,"
  but does not specify how the error is handled. A silent discard is acceptable;
  a silent discard with no `tracing::warn!` is an observability gap. Add a
  `tracing::warn!` at the call site for the failed-update path.

---

## 6. Suggestions

- **Consider adding `robots:view` to the seed role for `admin` immediately in
  commit 6.** The plan notes "seeded roles do not contain robot-specific caps in
  their defaults." That is correct policy — but the consequence is that a
  freshly bootstrapped server will have no role that can call
  `GET /api/admin/robots` without manual configuration. A note in the plan or a
  seed-roles update (even in commit 8) would help operators bootstrapping the
  feature.

- **`token new` with `--renew` flag against an expired token:** commit 7 tests
  "exactly one non-revoked row remains" after rotation. Consider also asserting
  the old row's `revoked` is explicitly `1`, not just that the count is 1. The
  partial unique index guarantees the count but does not guarantee the old row
  was the one set to revoked.

- **Commit 8 smoke sequence is useful but incomplete for the deactivate path.**
  The smoke commands in commit 8's validation do not include
  `cbc admin robots disable ci-test` followed by token authentication (should
  fail) followed by `cbc admin robots enable ci-test` (should succeed). Given
  that commit 6's deactivate semantics have been adjusted (see B1), explicitly
  exercising the disable/enable cycle in the smoke validation builds confidence
  in the change.

- **`display_identity()` wiring is listed in commit 6 for log call-sites across
  four route files.** This is mechanical but broad. Consider whether a clippy
  lint or a test that asserts no `tracing::info!` macro contains `user.email`
  directly (only `display_identity()`) would prevent regression in new handlers.

---

## 7. Open Questions

1. **What does `tombstone_robot` in `db/robots.rs` do to `users.active` vs
   `DELETE /api/admin/robots/{name}`?** The plan's DB function table says
   `tombstone_robot` sets `users.active = 0` and revokes all tokens. The route
   handler is `DELETE /api/admin/robots/{name}`. Should that route physically
   delete the `users` row, or set `active = 0`? The design preserves the row for
   FK chain integrity. The plan's DB function correctly uses soft-delete, but
   the naming (`tombstone_robot` / `DELETE` HTTP verb) should be reconciled in
   the spec to avoid confusion during implementation.

2. **`robot_tokens.expires_at = NULL` means "never expires" — is that
   explicit?** The schema allows `NULL`; the plan's validation clause "check
   `expires_at` (NULL or future)" implies it. Consider adding a brief note in
   commit 6's spec or the extractor description to make the NULL semantics
   explicit for the implementer.

3. **`POST /api/admin/robots/{name}/token` body flag `renew: bool` — is this a
   JSON body or a query parameter?** The plan lists it as a "body flag" but does
   not specify the content type. The rest of the API uses JSON bodies. Confirm
   that `renew` follows that convention and that the handler uses
   `Json<RotateTokenRequest>` rather than a query string.

---

## 8. Confidence Score

The scoring is applied to the plan document as a specification for future
implementation (not yet implemented code). Deductions reflect gaps that, if left
unresolved, will produce defects or review friction during implementation.

| Item                                                         | Points | Finding                                                                             |
| ------------------------------------------------------------ | ------ | ----------------------------------------------------------------------------------- |
| Starting score                                               | 100    |                                                                                     |
| D8: deactivate_entity spec deviates from design v3           | -5     | Robot deactivate should leave robot_tokens intact; plan revokes them                |
| D8: activate_entity returns 400 for all robot rows           | -5     | Breaks reversibility; disabled robots cannot be re-enabled without full revive      |
| D5: no test coverage for post-assignment forbidden-cap strip | -15    | Critical auth path — stripping at runtime after role update — has no test specified |
| D9: mark_used failure not logged                             | -5     | Silent discard on usage-update failure; no warn! specified                          |
| D8: commit 2 API break not flagged in commit 2 spec          | -5     | ?force removal is a server API contract break; noted only in commit 4 framing       |
| D11: write-only usage columns in commit 5 undisclosed        | -5     | Plan does not note that first_used_at/last_used_at have no reader until commit 6    |
| **Total**                                                    | **60** |                                                                                     |

**Interpretation: 60 — Significant issues. Must address before proceeding.**

The score is dominated by the forbidden-cap strip test gap (D5, -15) and the
two-part disable/enable semantic deviation (B1, two D8 deductions, -10 total).
The D5 deduction is high because the runtime cap-strip is the primary security
guarantee for robot tokens; an untested critical path in a security feature is a
high-severity specification gap. Once B1 and M2 are resolved in the plan, the
effective score rises to ~85 — acceptable with noted improvements.

---

## Review Summary

Three issues require plan revision before implementation begins:

1. **(B1, blocking — two changes)** Fix the disable/enable semantic deviation
   for robot targets in commit 6:
   - `deactivate_entity`: remove `revoke_all_tokens_for_robot`; keep only
     `users.active = 0` + cache purge.
   - `activate_entity`: return 400 only when the robot has no non-revoked
     `robot_tokens` rows (tombstoned state); for a merely disabled robot
     (`active = 0`, tokens intact), set `users.active = 1` normally.
   - Update the deactivate test assertion from `revoked = 1` to `revoked = 0`.
   - Add a disable/enable cycle test: deactivate → auth fails → activate → auth
     succeeds with original token.

2. **(M2, important)** Add a test case for the runtime forbidden-cap strip after
   a role update post-robot-creation. This is the primary runtime security
   guarantee and needs explicit test coverage in the plan.

3. **(M1, documentation)** Flag the `?force` parameter removal in commit 2's
   spec as a server API contract break, not a client-side UX change, and require
   the commit message to call it out.

Everything else is advisory. The plan is well-structured, the commit dependency
ordering is correct, and the codebase accuracy is high.
