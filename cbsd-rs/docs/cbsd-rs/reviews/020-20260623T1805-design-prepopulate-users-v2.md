# Design Review v2: Prepopulate Users (Pre-Provisioning)

| Field          | Value                                                                           |
| -------------- | ------------------------------------------------------------------------------- |
| Review seq     | 020                                                                             |
| Iteration      | v2                                                                              |
| Reviewer       | Staff Engineer (automated adversarial review)                                   |
| Review date    | 2026-06-23                                                                      |
| Design version | Draft v3                                                                        |
| Design doc     | `cbsd-rs/docs/cbsd-rs/design/020-20260623T1724-prepopulate-users.md`            |
| Prior review   | `cbsd-rs/docs/cbsd-rs/reviews/020-20260623T1736-design-prepopulate-users-v1.md` |

---

## Executive Summary

Draft v3 addresses all six v1 findings in good faith. The C1 (seed_admin
normalization), S1 (domain-check extraction), S2 (deactivated-human path), S3
(validate-after-normalize ordering), and S4 (sqlx cache + cbc forward-compat)
fixes are correct and close those findings. The v1 C2 severity assessment
("silent 201") was factually wrong: empirically confirmed via Python's `sqlite3`
module, `INSERT OR IGNORE` under `PRAGMA foreign_keys = ON` raises
`SQLITE_CONSTRAINT_FOREIGNKEY` immediately — it does NOT suppress FK violations.
The real failure mode is a loud 5xx, not a silent success; v3 corrects this, and
C2 is downgraded accordingly.

However, v3 introduces one new critical issue: the proposed boundary-#4
normalization (lowercase the `{email}` path parameter on all existing admin
endpoints) will break management of any robot whose name contains uppercase
letters. Robot names are stored verbatim in the `robot+{name}@robots` synthetic
email; lowercasing that key produces a non-existent lookup and a 5xx rather than
the expected 404 or 200. `validate_robot_name` explicitly accepts uppercase
(`is_ascii_alphanumeric` matches `A-Z`), so uppercase robot names are a
supported and valid production state. The fix requires either scoping boundary
#4 to human-identity emails only, or routing robot management through the
name-based endpoint that already handles this correctly.

A second pre-existing bug exposed by v3 analysis — `add_entity_role` silently
defaults to `target_is_robot = false` when `get_user` returns `None` and then
proceeds to `add_user_role`, which fails with a FK 5xx — is called out as a
significant concern. It is pre-existing, but boundary #4 normalization directly
reduces its "mismatched case" trigger, making it the right moment to document
and fix.

---

## Finding Dispositions from v1

### C1 — seed_admin not normalized before seed INSERT

**Status: FIXED (design)**

v3 correctly enumerates `seed_admin` config load as boundary #2 and explains the
exact failure mode (mixed-case `seed_admin` forks a zero-cap admin row at first
login). The fix is correctly placed: normalize in `config.rs` before the value
is used anywhere, so both `run_first_startup_seed` and the dev-mode login path
see the canonical form. The current code in `config.rs` `SeedConfig::validate()`
does not yet implement normalization — this is expected for a design document;
the design correctly identifies what must change.

One minor gap: `SeedConfig` is a plain `#[derive(Deserialize)]` struct. The
design says "normalized at config load" but does not specify the mechanism.
`serde` deserializes fields directly into the struct with no post-processing
hook, so normalization requires either a post-deserialization mutation step (a
`fn post_load(mut self) -> Self` called from `load_config`), a custom
`Deserialize` impl, or a newtype. This is an implementation detail the plan
document should resolve explicitly, but it does not affect design correctness.

The proposed seed test (a mixed-case `seed_admin` config produces exactly one
`users` row) is a correct regression specification.

---

### C2 — verbatim-match admin endpoints / INSERT OR IGNORE behavior

**Status: PARTIALLY FIXED — downgraded to Significant (was Critical)**

**Mechanism correction confirmed.** The v1 review's "silent 201" claim was
incorrect. Empirical test result (Python `sqlite3`, same C engine as sqlx):

```
python3 -c "
import sqlite3
c = sqlite3.connect(':memory:')
c.execute('PRAGMA foreign_keys=ON')
c.execute('CREATE TABLE users(email TEXT PRIMARY KEY, name TEXT NOT NULL)')
c.execute('CREATE TABLE user_roles(user_email TEXT NOT NULL REFERENCES users(email), role_name TEXT NOT NULL, PRIMARY KEY(user_email,role_name))')
try:
    cur = c.execute(\"INSERT OR IGNORE INTO user_roles VALUES('ghost@x','admin')\")
    print('NO ERROR rowcount', cur.rowcount, 'count', c.execute('SELECT count(*) FROM user_roles').fetchone()[0])
except Exception as e:
    print('ERROR', type(e).__name__, e)
"
```

Output: `ERROR IntegrityError FOREIGN KEY constraint failed`

`INSERT OR IGNORE` suppresses `SQLITE_CONSTRAINT_UNIQUE` (error code 2067) but
not `SQLITE_CONSTRAINT_FOREIGNKEY` (error code 787). Under
`PRAGMA foreign_keys = ON`, an FK violation on a non-existent `user_email` is
raised immediately and propagates as a 5xx. v3's paragraph — "the real failure
mode is a loud FK error / zero-row UPDATE, not a silent grant" — is accurate.

**Residual gap.** The v3 fix (normalize operator-supplied emails at boundary #4)
corrects the case-mismatch scenario. The pre-existing `add_entity_role` bug
(lines 1183–1190, `admin.rs`) remains and is called out under the new
Significant Concern N1 below.

---

### S1 — domain check not extracted

**Status: FIXED**

v3 explicitly states that `is_email_domain_allowed` is new work extracted from
the inline block in `validate_user_info` (lines 90–95 of `auth/oauth.rs`).
Current code confirms the extraction does not yet exist; the design correctly
characterizes this as future work, not pre-existing infrastructure.

---

### S2 — deactivated-human re-provision path

**Status: FIXED**

v3 documents `AlreadyExists` for all existing `is_robot = 0` rows regardless of
the `active` flag, with the remediation path (`/activate` then role endpoints)
spelled out. The deliberate asymmetry with robot `create_or_revive` (which does
reactivate tombstoned robots) is noted and justified.

---

### S3 — validate on normalized value

**Status: FIXED**

v3 is explicit: "Validation runs on the normalized email and the provided name —
the request handler normalizes the email first, then every check below operates
on the canonical value." The robot-email guard and domain check cannot be
bypassed by case. Design is correct.

---

### S4 — sqlx cache + cbc forward-compat

**Status: FIXED**

Both items addressed: `cargo sqlx prepare --workspace -- --all-targets` is in
the implementation notes (with the explanation of why `--all-targets` is
required), and `#[serde(default)] first_login_at: Option<i64>` is specified for
the cbc `UserWithRoles` deserialization struct.

---

## Critical Issues

### C1 (new) — Boundary #4 normalization breaks uppercase-named robots

**Severity: Critical**

**Problem.** v3 boundary #4 states: normalize the `{email}` path parameter on
all existing admin endpoints (`/api/admin/entity/{email}/…` and the
`revoke-all-tokens` body). The intent is to allow operators to use human emails
in any case. However, these endpoints are shared by robots and humans — they
branch on `target.is_robot` after the lookup. Robot identities are stored as
`robot+{name}@robots`, where `{name}` is verbatim from `validate_robot_name`.

`validate_robot_name` permits uppercase letters:

```rust
let is_border = |c: char| c.is_ascii_alphanumeric() || c == '_';
// is_ascii_alphanumeric() matches [a-zA-Z0-9], including A-Z
```

A robot named `CI` is stored as `users.email = "robot+CI@robots"`. Applying
`normalize_email` (lowercasing) to the path parameter before the `get_user`
lookup transforms `"robot+CI@robots"` to `"robot+ci@robots"`, which does not
match any row. The subsequent behavior depends on the endpoint:

- `deactivate_entity` / `activate_entity` — `.ok_or_else(NOT_FOUND)` → 404.
  Correct-looking error, but the robot exists and cannot be managed.
- `add_entity_role` — `unwrap_or(false)` → proceeds to `add_user_role` with the
  lowercased key → FK error → 5xx. The robot is unmanageable and the operator
  sees a confusing 500.
- `replace_entity_roles` — passes lowercased email into `set_user_roles` → empty
  DELETE + FK error on INSERT → 5xx.

**Impact.** Any robot created with at least one uppercase letter in its name
becomes unmanageable via the shared entity endpoints after boundary #4
normalization ships. This is a functional regression introduced by the v3 fix
for C2.

**Recommendation.** Do not apply `normalize_email` blindly to the `{email}` path
parameter on entity endpoints. Two safe alternatives:

Option A (simplest, minimal diff): look up the entity first _without_
normalization. If found and `is_robot = 0`, proceed with the normalized form for
any subsequent human-email operations. If `is_robot = 1`, use the verbatim form.
If not found at all, retry with the lowercased form — if that finds a human row,
proceed; if still not found, 404.

Option B (cleaner API contract): treat human and robot management as distinct
surfaces. Separate `/api/admin/entity/{email}` into `/api/admin/user/{email}`
(always normalize) and `/api/admin/robot/{name}` (use `name_to_synthetic_email`
directly, no normalization of the path segment). The existing
`/api/admin/robots` create endpoint already uses the name-based surface. This
aligns with the design's own statement that robots have their own lifecycle
endpoint.

Either option must be reflected in both the design and the test matrix. The
route test for "operator-supplied mixed-case email resolves to the same
lowercased user" must explicitly cover only the human path; a complementary test
for "operator-supplied robot name resolves correctly without normalization"
should be added.

---

## Significant Concerns

### S1 (new) — `add_entity_role` proceeds to `add_user_role` when entity does not exist

**Severity: Significant**

**Problem.** In `routes/admin.rs` lines 1183–1190:

```rust
let target_is_robot = db::users::get_user(&state.pool, &email)
    .await
    .map_err(…)?
    .map(|u| u.is_robot)
    .unwrap_or(false);  // ← None (entity not found) maps to false
```

When `get_user` returns `None` (entity does not exist), `target_is_robot` is
`false` and the handler proceeds to line 1211: `db::roles::add_user_role(…)`.
Under `PRAGMA foreign_keys = ON`, this INSERT violates the FK constraint and
surfaces as a 5xx `"failed to add entity role"`. The client receives no 404 — it
receives a 500 for a condition that is simply "entity does not exist."

The sibling handlers `deactivate_entity` (line 94) and `activate_entity` use
`.ok_or_else(NOT_FOUND)` correctly. `add_entity_role` is the only handler in
this group that omits the existence check.

This is a pre-existing bug, not introduced by v3. However, boundary #4
normalization is the designated fix for the case-mismatch scenario that was the
most common trigger: after normalization ships, the remaining trigger is a
genuinely non-existent email, which an operator would expect to produce a 404.
The plan commit for boundary #4 normalization is the natural moment to fix this
too.

**Recommendation.** Change the `get_user` call in `add_entity_role` from
`unwrap_or(false)` to
`.ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "entity not found"))?`. This
is a one-line change and makes the behavior consistent with `deactivate_entity`
and `activate_entity`. Add it to the normalization commit or as a paired fix
commit.

---

## Minor Observations

### N1 — `SeedConfig` normalization mechanism unspecified

The design says normalize `seed_admin` at config load but does not specify the
implementation mechanism. `SeedConfig` is a plain
`#[derive(Debug, Default, Deserialize)]` struct; `serde` runs no post-hooks. The
implementation will need an explicit post-load mutation step in `load_config`
(e.g.,
`config.seed.seed_admin = config.seed.seed_admin.map(|e| normalize_email(&e))`)
or a newtype. Neither is complex, but the plan document should name the approach
to avoid ambiguity during implementation.

---

### N2 — dev-mode callback must normalize `params.dev_email`

`routes/auth.rs` `callback` handler (lines 244–253) constructs a
`GoogleUserInfo` from the `dev_email` query parameter. Boundary #1 says "before
`create_or_update_user`"; the design does not call out that `params.dev_email`
specifically must be lowercased before building the struct. In practice,
`dev_email` is populated from `seed_admin` (which boundary #2 normalizes), so
the common path is safe. But an operator who manually constructs a dev-mode URL
with a mixed-case email bypasses that. The code change note for the OAuth
callback should explicitly mention normalizing `params.dev_email` before it is
used in the struct constructor.

---

### N3 — `ProvisionOutcome` is a single-variant enum

`ProvisionOutcome::Created` has exactly one variant. A single-variant enum that
carries no associated data is equivalent to `()`. This is not a bug, but if a
follow-up adds `AlreadyExistsWithSameRoles` or a similar "soft match" outcome,
the enum pays off. If no expansion is anticipated, returning `()` from
`provision_user` on success and using the error enum for all failure modes is
more idiomatic. Leave as-is if the design anticipates future variants; otherwise
note it in the plan.

---

### N4 — `EntitySummary` and `UserWithRoles` responses in `cbc`

The design specifies `first_login_at` on both the server-side `EntitySummary`
and the cbc `UserWithRoles` struct. Current code in `cbc/src/admin/users.rs`
(lines 111–119) does not yet have this field (expected). The design's note that
`is_robot = 0` must gate the "pending" display is correct — robots always carry
`first_login_at = NULL` and must not be labeled pending. This constraint should
be expressed as a test case in the cbc display logic, not just a prose note.

---

## Strengths

- **Empirical failure-mode correction.** v3 correctly replaces the inaccurate
  "silent 201" framing with the accurate "loud 5xx / zero-row update." The
  revision history calls out the prior error explicitly, which aids future
  readers.
- **Isolation discipline from `create_or_revive`.** The `BEGIN IMMEDIATE` +
  re-read-under-lock pattern directly mirrors the existing robot implementation.
  This is the correct isolation discipline for SQLite under concurrent admin
  operations and avoids reinventing it.
- **Backfill rationale.** The v2/v3 decision to backfill
  `first_login_at = created_at` for existing human rows is well-reasoned:
  login-created rows have `created_at` equal to their actual first-login time,
  so the backfill is accurate. The one acknowledged inaccuracy (a
  bootstrapped-but-never-logged-in seed_admin is "un-pended") is an honest edge
  case with a clear acceptance rationale.
- **Domain validation at provisioning.** Rejecting addresses that cannot
  authenticate is the right gate — without it, an admin could provision accounts
  that are silently uncacheable. Extracting `is_email_domain_allowed` as a
  shared helper is the correct factoring.
- **Test matrix completeness.** The proposed seed test (mixed-case `seed_admin`
  → one row, wildcard retained), the route test for mixed-case operator email,
  and the DB unit tests for `first_login_at` lifecycle cover the main regression
  targets.
- **`ON UPDATE CASCADE` absence noted.** The migration compatibility section
  correctly calls out that `user_roles`, `tokens`, `api_keys`, and
  `builds.user_email` all reference `users.email` without `ON UPDATE CASCADE`,
  making a PRIMARY KEY rename migration risky. This is why ingress normalization
  (not `COLLATE NOCASE`) is the right approach.

---

## Open Questions

1. **Are uppercase robot names present in production?** The critical issue above
   (C1 new) is triggered only if any robot was created with an uppercase name.
   The same pre-deployment verification query used for humans
   (`SELECT email FROM users WHERE email <> lower(email)`) will reveal this for
   robots too. Run it before implementation begins.

2. **Option A vs Option B for entity endpoint scoping?** The two options in the
   C1 new recommendation have different blast radii. Option A (conditional
   normalization in the handler) is lower-risk but adds per-handler branching.
   Option B (split the human/robot surfaces) is cleaner long-term and aligns
   with how the creation surface is already split, but is a larger API change.
   Which path does the team prefer? This decision drives the plan commit
   structure.

3. **Should `add_entity_role`'s missing 404 be a standalone commit or bundled?**
   It is a one-line fix, pre-existing, and safe to ship independently. Bundling
   it into the normalization commit is also clean (both touch the same handler).
   Resolve before writing the plan.

---

## Confidence Score

Applying the confidence-scoring rubric to the **design document** (not the
implementation):

| Item                                      | Points | Description                                                                                                                                                                                                                                                                            |
| ----------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                            | 100    |                                                                                                                                                                                                                                                                                        |
| D1: boundary-#4 spec incomplete           | -20    | Robot path param normalization is underspecified — the design mandates lowercasing the shared `{email}` param without carving out robot synthetic emails, which breaks management of uppercase-named robots. This is deferred work that must be resolved before implementation begins. |
| D11: `SeedConfig` normalization mechanism | -5     | Design says "normalize at config load" but does not specify how, given `serde`'s pure deserialization model. Implementation-blocking ambiguity.                                                                                                                                        |
| D11: dev-mode `dev_email` not called out  | -5     | The callback code-change note omits the specific `params.dev_email` normalization step; operator-crafted dev URLs bypass boundary #2's guarantee.                                                                                                                                      |
| **Total**                                 | **70** |                                                                                                                                                                                                                                                                                        |

**Interpretation: Significant issues. Must address boundary-#4 robot scope
before proceeding to implementation.** The remaining deductions are minor
implementation-note gaps that do not block the design model itself.

The previous v1 score was 62 (two Critical findings). v3 closes five of six v1
findings correctly. The score rises to 70, held down by the new Critical
introduced by the very normalization fix that closed C2.

---

## Verdict

**Revise and re-review.**

The provisioning model, migration strategy, normalization rationale, and
`first_login_at` semantics are sound. The v3 revisions to C1, S1–S4 are correct.

The single blocker is the new Critical: boundary #4 normalization as specified
will silently break management of uppercase-named robots. This must be resolved
in the design — either by scoping the normalization to human-identity inputs
only, or by routing robot management through a name-based surface that bypasses
the email normalizer — before the plan commit that implements boundary #4 is
written.

Once the robot scoping decision is made and reflected in the design, and the
`add_entity_role` 404 fix is noted in the plan, the design is otherwise ready to
implement.
