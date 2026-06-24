# Design Review v3 — Prepopulate Users (Pre-Provisioning)

| Field         | Value                                                        |
| ------------- | ------------------------------------------------------------ |
| Design        | 020                                                          |
| Design file   | `docs/cbsd-rs/design/020-20260623T1724-prepopulate-users.md` |
| Plan file     | `docs/cbsd-rs/plans/020-20260623T2118-prepopulate-users.md`  |
| Design status | Draft v5                                                     |
| Review        | v3 (third pass)                                              |
| Date          | 2026-06-23                                                   |
| Reviewer      | Staff reviewer (adversarial)                                 |
| Prior review  | `020-20260623T1805-design-prepopulate-users-v2.md`           |

---

## Executive Summary

Design v5's core decision — all identities are lowercase; robot names obey the
same rule; v4 carve-out dropped — cleanly eliminates the v2 Critical finding
(boundary #4 breaking uppercase robots) with a simpler invariant than the
carve-out it replaces. The provisioning model, concurrency discipline, migration
guard, and first-login tracking are all sound.

Two significant design-text defects must be corrected before commit 2 is
written, because both will cause wrong first cuts. First: boundary #5 states
that lowercasing happens "before `name_to_synthetic_email`," but in
`create_or_revive_robot` the validation call (`validate_robot_name`) runs
_before_ `name_to_synthetic_email`. An implementer who follows the design's
stated anchor will lowercase after validation, which means `validate_robot_name`
receives the un-lowercased name first and now rejects it — breaking the create
path while the design promises it will work. Second: the design claims that
tightening `validate_robot_name` to lowercase-only "trips any future caller that
builds a name without normalizing," but `validate_robot_name` is called only in
the `create_or_revive_robot` handler; every lookup handler (`get_robot`,
`tombstone_robot`, `create_or_rotate_token`, `revoke_robot_token`,
`set_robot_description`) bypasses it entirely. The claimed backstop does not
protect the lookup paths.

In addition, the plan omits `set_robot_description`
(`PUT /api/admin/robots/{name}/description`) from boundary #5 coverage in
commit 2. This handler constructs a synthetic email directly via
`name_to_synthetic_email` at line 784 of `routes/robots.rs`; without lowercasing
the path param first, an uppercase robot name in this endpoint yields a
silent 404.

One file-target error is present in the plan: the commit 2 table attributes
robot-handler work to `routes/admin.rs`; all robot handlers live in
`routes/robots.rs` (admin.rs only mounts the sub-router via `.nest`).

---

## Part I — Design v5 Delta Review

### Critical Issues

None. The v2 Critical finding (boundary #4 uniformly lowercasing `{email}`
breaks uppercase robots) is resolved by the v5 decision to make all robot
identities lowercase. With the constraint that no production robot has an
uppercase name (the operational guard query covers robots), this is a clean
resolution with no migration risk.

### Significant Concerns

**S1 — Lowercase-before-validate ordering: design anchor is wrong**

The design (Normalization section, boundary #5) says to lowercase the robot name
at ingress "before `name_to_synthetic_email`." In `create_or_revive_robot`
(routes/robots.rs:207), the call sequence is:

```
line 219: validate_robot_name(&body.name)     ← validation first
line 261: name_to_synthetic_email(&body.name) ← synthetic email second
```

An implementer who reads "lowercase before `name_to_synthetic_email`" and
applies the lowercase transform between lines 219 and 261 will not lowercase
before the validation call. After v5 tightens `validate_robot_name` to
`[a-z0-9_.-]`, `validate_robot_name("CI")` returns `Err` — so
`POST /api/admin/robots` with `{"name": "CI"}` returns 400, directly
contradicting the design's own promise: "since ingress lowercases first, this
never rejects a real request on the case dimension" and its test "robot `CI`
stored as `robot+ci@robots`."

The only implementation that is consistent with _both_ the tightened validator
and the handler tests is lowercase _before_ `validate_robot_name`. The design
must change "before `name_to_synthetic_email`" to "before `validate_robot_name`"
in boundary #5. The plan's commit 2 table row for `routes/admin.rs (robots)`
repeats the same phrase and must match the correction.

This is a concrete implementation-direction error, not an ambiguity. Any
engineer following the document as written will introduce a bug.

**S2 — `validate_robot_name` backstop claim is false for lookup handlers**

Design text (Normalization section, paragraph following boundary #5):

> `validate_robot_name` is tightened to lowercase-only (`[a-z0-9_.-]`); since
> ingress lowercases first, this never rejects a real request on the case
> dimension — it documents the invariant and **trips any future caller that
> builds a name without normalizing**.

`validate_robot_name` is called in exactly one place: `create_or_revive_robot`
(routes/robots.rs:219). The five lookup handlers reach the db layer directly:

| Handler                  | Line | Path to synthetic email                      |
| ------------------------ | ---- | -------------------------------------------- |
| `get_robot`              | 433  | `get_robot_by_name(&state.pool, &name)`      |
| `tombstone_robot`        | 517  | `get_robot_by_name(&state.pool, &name)`      |
| `create_or_rotate_token` | 596  | `rotate_token(&state.pool, &name, …)`        |
| `revoke_robot_token`     | 705  | `get_robot_by_name(&state.pool, &name)`      |
| `set_robot_description`  | 771  | `name_to_synthetic_email(&name)` at line 784 |

None of these call `validate_robot_name`. A future engineer adding a sixth
lookup handler who forgets to lowercase gets a silent 404, not a compile-time or
runtime validation error — exactly the scenario the claimed backstop is supposed
to prevent.

The safer structure is to lowercase inside `name_to_synthetic_email` itself
(`format!("robot+{}@robots", name.to_lowercase())`), which makes every call site
— server and `cbc` alike — automatically normalized with no per-handler
discipline required. Note: this alone does not fix S1, because the create path
must still lowercase _before_ `validate_robot_name`; that step is in the
handler, not in `name_to_synthetic_email`. Alternatively, if the per-handler
approach is retained, the design must (a) remove the false "trips any future
caller" safety claim and (b) enumerate all six handlers explicitly as per-commit
work targets.

### Minor Observations

**M1 — `cbc` local `name_to_synthetic_email` and display drift**

`cbc/src/admin/robots.rs` has its own copy of `name_to_synthetic_email` (line
349). The seven call sites (lines 604, 640, 676, 701, 726, 747, 776) pass
`&args.name` verbatim to construct the `{email}` path param for entity
sub-commands. Server-side boundary #4 normalizes the path param on arrival, so
round-trip correctness holds. The only residual is that `cbc` output (e.g., the
displayed synthetic email in a robot list) may reflect the un-lowercased name
the operator typed rather than the stored lowercase form — cosmetic drift, not a
functional bug. Not a blocker; could be a minor follow-up.

**M2 — `set_robot_description` missing from plan boundary #5 (see Part II)**

This is a plan coverage gap; see S1 in the Plan Review below.

**M3 — `display_name` round-trip is correct given S1 fix**

`create_robot_in_conn` (db/robots.rs:384-385) derives `display_name` as
`format!("robot:{name}")` using the `name` argument directly. Provided the
caller (the handler) lowercases before calling the db function (as required by
the S1 fix), `display_name` will be `"robot:ci"` — consistent with the design
claim. No action needed beyond ensuring S1 is applied to the handler; the db
layer does not need modification.

---

## Part II — Plan Review

### Significant Concerns

**P-S1 — `set_robot_description` missing from boundary #5 in commit 2**

Commit 2's table row for "routes/admin.rs (robots)" lists boundary #5 coverage
as: "POST /api/admin/robots and the /api/admin/robots/{name} lookups (get, token
rotate, delete)." The router for the robot handlers also includes
`set_robot_description` (`PUT /api/admin/robots/{name}/description`). Its
handler at routes/robots.rs:771 does:

```rust
// line 784
let email = db::robots::name_to_synthetic_email(&name);
```

There is no lowercasing of `name` before this call. Without it,
`PUT /api/admin/robots/CI/description` constructs `robot+CI@robots`, misses the
stored `robot+ci@robots` row, and returns 404 (the
`if !updated { return NOT_FOUND }` guard at the bottom of the handler fires).
The plan must add this handler to the boundary #5 enumeration and to the commit
2 change table.

**P-S2 — Wrong file target for robot-handler work**

Commit 2's table has this row:

```
cbsd-server/src/routes/admin.rs (robots) | Lowercase the robot name …
```

No robot handler is in `routes/admin.rs`. `admin.rs:49` mounts the sub-router
with `.nest("/robots", robots::router())`; every robot handler
(`create_or_revive_robot`, `get_robot`, `tombstone_robot`,
`create_or_rotate_token`, `revoke_robot_token`, `set_robot_description`) lives
in `routes/robots.rs`. The table row must reference the correct file.

### Minor Observations

**P-M1 — Commit 2 LOC estimate is defensible**

At ~450 LOC, commit 2 spans multiple files but delivers a single invariant: all
boundaries lowercase before any db call. Splitting at the human/robot seam would
produce an intermediate state where some boundaries normalize and some do not,
violating the invariant for any test that crosses both. Keeping it together is
the correct call; the estimate is inside the 400–800 guided range.

**P-M2 — Commit 4 domain-helper placement is safe**

Extracting `is_email_domain_allowed` from `validate_user_info` in commit 4
rather than commit 2 is safe. The login path continues to use its inline domain
check through commits 2 and 3; commit 4 extracts it as a behavior-preserving
refactor while adding the provisioning handler that needs the helper. No
functional regression between commits 2 and 4.

**P-M3 — `-- --all-targets` noted in verification section**

The verification section correctly includes `-- --all-targets` for
`cargo sqlx prepare`. No action needed.

---

## Prior-Review Carry-Over Check

| Finding                                    | Status                                    |
| ------------------------------------------ | ----------------------------------------- |
| v2 C1: boundary #4 breaks uppercase robots | Resolved — v5 all-lowercase invariant     |
| v2 S1: `add_entity_role` missing 404       | Addressed — design 197-202, plan commit 2 |

---

## Strengths

- The all-lowercase invariant is simpler and more correct than the v4 carve-out
  it replaces. Eliminating the robot special-case from boundary #4 is the right
  call; the resulting rule is one sentence instead of two paragraphs.
- The operational guard (`SELECT email FROM users WHERE email <> lower(email)`)
  explicitly covers robots (they live in `users`) and the FK caveat on
  `robot_tokens.robot_email` is correctly acknowledged.
- `BEGIN IMMEDIATE` + re-read-under-lock concurrency pattern correctly mirrors
  the established robot creation discipline. Concurrent provisioning requests
  serialize cleanly.
- The `first_login_at` backfill rationale (`created_at` = true first-login time
  for login-created rows, with the accepted seed_admin edge case) is precisely
  argued and correct.
- Workers (`workers` table, `api_keys`) are correctly identified as separate
  from robot identities (`users` table with `is_robot = 1`). The lowercase
  change has zero bearing on worker seeding.
- Idempotency decision (provision = create; no merge-on-conflict) is clearly
  justified and mirrors robot semantics.

---

## Open Questions

1. **`name_to_synthetic_email` centralization (S2):** Will the implementation
   lowercase inside `name_to_synthetic_email`, or apply per-handler transforms?
   Centralizing is safer long-term. If per-handler is chosen, the design must
   remove the "trips any future caller" claim and enumerate all six handlers
   explicitly.

2. **`cbc` robot command lowercasing (M1):** Out of scope for this feature, or
   should `cbc` normalize robot names locally so CLI output reflects stored
   form?

---

## Confidence Scores

### Design v5

| Criterion             | Score | Notes                                                                       |
| --------------------- | ----- | --------------------------------------------------------------------------- |
| Correctness           | 7/10  | Sound model; S1 phrasing misleads implementation                            |
| Completeness          | 7/10  | S2 false backstop claim; `set_robot_description` gap                        |
| Distributed soundness | 9/10  | Concurrency pattern correct                                                 |
| Security              | 9/10  | Guards, domain check, audit logging in place                                |
| Operational soundness | 9/10  | Guard query, backfill rationale explicit                                    |
| Maintainability       | 7/10  | S2: no central normalize choke point = future per-handler discipline burden |

**Overall: 8/10** — Design model is sound; two significant text defects and a
plan omission must be corrected before implementation.

---

## Verdict

**Approve with conditions.** The v5 design decision (all-lowercase identity) is
correct and the provisioning model is ready. Three items must be fixed in the
design and plan before commit 2 is written:

1. **S1 (design + plan):** Change "before `name_to_synthetic_email`" to "before
   `validate_robot_name`" in boundary #5 description. The correct lowercasing
   anchor is before validation, not before the synthetic-email call.

2. **S2 (design):** Either (a) remove the "trips any future caller" backstop
   claim and replace it with an explicit enumeration of all six per-handler
   normalization sites, or (b) move the lowercasing into
   `name_to_synthetic_email` itself (and document that the create handler still
   needs to lowercase _before_ `validate_robot_name`).

3. **P-S1 + P-S2 (plan commit 2):** Add `set_robot_description` to boundary #5
   coverage; correct the file target from `routes/admin.rs` to
   `routes/robots.rs`.
