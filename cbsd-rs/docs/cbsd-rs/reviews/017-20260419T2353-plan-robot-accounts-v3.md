# Plan Review: Robot Accounts — v3

**Document:** `017-20260419T2123-robot-accounts.md`\
**Reviewer:** Claude Sonnet 4.6\
**Date:** 2026-04-19\
**Prior scores:** v1 = 60 (1 blocker), v2 = 87 (2 minor gaps)

---

## Summary Assessment

The v3 plan correctly resolves both v2 residuals. The `BEGIN IMMEDIATE`
isolation rationale is now precise — the plan names the exact SQLite error code
that would fire under a deferred transaction and explains why the immediate-mode
serialisation closes the gap entirely. The `DELETE .../token` handler now
carries a clean 404/200 decision tree backed by matching test cases. One minor
gap remains: the nonexistent-name test matrix covers three mutating endpoints
but omits `GET /api/admin/robots/{name}`, leaving its 404 behaviour untested
despite the plan asserting the policy applies to every
`/api/admin/robots/{name}/*` handler.

**Recommendation: Go.** The plan is ready to implement. The residual is
non-blocking and can be closed inside commit 7 itself.

---

## Verification of v2 Findings

### n1 — `rotate_token` concurrent-500 (v2 minor gap)

**Resolution: Correct.**

Plan line 546 asserts: _"Caller opens the transaction with `BEGIN IMMEDIATE`."_
The key-details section (lines 556–567) explains the mechanism:

> Two simultaneous `POST .../token` calls are serialised at lock acquisition;
> the loser waits for the winner to commit, then re-reads and proceeds against
> the post-revoke state. `SQLITE_CONSTRAINT_UNIQUE` (code 2067) is avoided
> entirely — there is never a moment where two non-revoked rows coexist and the
> unique index fires.

The concurrent-rotation test (lines 595–600) operationalises this:

> Both complete successfully with distinct `token_hash` values; after both
> commit, exactly one row has `revoked = 0`. Neither request returns 500.

The `rotate_token(tx, robot_email, new_hash, new_prefix, expires_at)` signature
takes the transaction as a caller-supplied parameter, making the contract
explicit in the function boundary. The plan's rationale is complete and
testable.

### n2 — `DELETE .../token` 404 vs 200 (v2 minor gap)

**Resolution: Correct.**

Handler spec line 548: _"Loads the robot row first; returns **404** if the name
resolves to no `users` row with `is_robot = 1`; otherwise proceeds to revoke any
non-revoked tokens and returns **200**, with response body indicating the row
count revoked so a no-op is distinguishable from a real revoke."_

The decision tree at lines 610–623 enumerates both branches:

- Existing robot, no non-revoked tokens → 200, `{"revoked": 0}`
- Nonexistent robot name → 404

Tests at lines 624–629 cover `POST /token`, `DELETE /token`, and
`PUT /description` for nonexistent names. This is a meaningful improvement over
the v2 plan, which left the 404 path implicit.

---

## Net-New Findings

### Minor: `GET /api/admin/robots/{name}` 404 omitted from test matrix

The plan states (line 622): _"This policy applies to every
`/api/admin/robots/{name}/*` handler."_ The handler description at line 296
specifies `robots:view` permission but does not mention 404 for unknown names.
The nonexistent-name test matrix at lines 624–629 covers three of the four
`{name}`-scoped handlers (POST, DELETE, PUT) but omits GET.

The 404 behaviour is implicit from the "every handler" assertion, but the
missing test leaves it unverified. This is the only net-new finding relative to
the v2 review; it is non-blocking.

**Resolution:** Add one test case to the matrix: `GET /api/admin/robots/{name}`
with a nonexistent name → 404.

---

## Confidence Score

| Item                                            | Points | Description                                                                                                    |
| ----------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------- |
| Starting score                                  | 100    |                                                                                                                |
| D5: GET 404 not in nonexistent-name test matrix | -5     | Plan line 622 asserts the 404 policy covers every `{name}`-scoped handler; the test at lines 624–629 skips GET |
| **Total**                                       | **95** |                                                                                                                |

**Interpretation:** 95 — Ready to proceed. Minor issue only.

---

## Go / No-Go

**Go.**

The plan is implementable as written. The single residual (D5, -5) is confined
to a single missing test case in commit 7. It can be added without structural
changes to the handler or the store layer.

**Residual action (non-blocking):**

- Commit 7 test matrix: add `GET /api/admin/robots/{unknown-name}` → 404 to the
  nonexistent-name coverage table (lines 624–629).
