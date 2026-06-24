# Design Review — Prepopulate Users (Pre-Provisioning)

| Field          | Value                                                 |
| -------------- | ----------------------------------------------------- |
| Reviews design | 020 (`design/020-20260623T1724-prepopulate-users.md`) |
| Type           | design                                                |
| Iteration      | v1                                                    |
| Date           | 2026-06-23                                            |
| Reviewer       | Staff Engineer (adversarial review)                   |
| Verdict        | Revise and re-review                                  |
| Confidence     | 62 / 100                                              |

## Scope

Adversarial review of the Draft v2 design for pre-provisioning human users in
`cbsd-rs`. Every claim in the design was checked against the actual source:
`db/users.rs`, `db/robots.rs`, `db/roles.rs`, `db/seed.rs`, `routes/auth.rs`,
`routes/admin.rs`, `auth/oauth.rs`, `auth/extractors.rs`, `config.rs`,
`main.rs`, and migrations `001`/`005`/`007`/`009`.

The core idea is sound and aligns well with the existing robot-account pattern.
Two findings block implementation as written: a **bootstrap identity split**
caused by the un-normalized `seed_admin` config value, and a **false
load-bearing claim** that all email lookups derive from the two ingress points
(they do not — every sibling admin endpoint matches an operator-supplied email
verbatim, and one of them fails _silently_). Both are concrete, both contradict
statements the design makes, and both must be resolved before coding.

## Executive Summary

The data-model reuse is correct, the `BEGIN IMMEDIATE` + re-read concurrency
design is the right pattern, and the `first_login_at` backfill reasoning is
valid. But the design's central simplifying claim — "normalize at exactly two
ingress points and everything downstream stays consistent" — is false in the
current codebase. Multiple admin-facing endpoints take an email as a fresh path
or body parameter and match it verbatim against the stored (now-lowercased)
identity. One of them (`add_entity_role`) fails as a **silent success**.
Separately, the dev-mode login path pipes the raw `seed_admin` config string
into the callback, so a mixed-case `seed_admin` is _guaranteed_ to fork a
zero-capability second identity once the callback lowercases — and the proposed
operational guard does not inspect the config value, only existing rows. Neither
finding causes corruption on _today's_ all-lowercase deployment, which is why
the deployment is shippable, but both are latent footguns that the design
explicitly claims it has eliminated. Fix the framing and the normalization
placement, then re-review.

## Critical Issues 🔴

### C1 — `seed_admin` config value is never normalized; mixed-case seed forks a zero-cap admin

**Problem.** `db::seed::run_first_startup_seed` (seed.rs:140–155) inserts
`config.seed.seed_admin` (`config.rs:159`, `Option<String>`) into `users`
_verbatim_ — no lowercasing. The design normalizes email at exactly two ingress
points (OAuth callback + provisioning handler) and explicitly lists "Config-file
seeding … is unchanged" as a non-goal. But dev-mode login feeds `seed_admin`
straight into the callback as `dev_email`:

- `login` handler, auth.rs:186–192:
  `format!("/api/auth/callback?state={oauth_nonce}&dev_email={email}")` where
  `email = config.seed.seed_admin`.
- `callback` handler, auth.rs:244–253: constructs `GoogleUserInfo { email, … }`
  directly from `dev_email`.

The design says to normalize "both the Google path **and the dev-mode
short-circuit**." Once that normalization lands, the callback will lowercase
`Admin@Corp.com` to `admin@corp.com` and call `create_or_update_user` with the
lowercased form. The seed row is `Admin@Corp.com`. `ON CONFLICT(email)` does
**not** match (the PK is case-sensitive `TEXT`), so a **second** `users` row is
INSERTed with `admin@corp.com` and **zero roles** — the admin role lives only on
the original mixed-case row. The operator logs in and has no capabilities.

**Impact.** Lockout / privilege loss on the single most important account in the
system. The "operational guard" the design proposes
(`SELECT email FROM users WHERE email <> lower(email)`) does not catch this: at
first startup the `seed_admin` row may already be mixed-case _and pass the
all-lowercase check trivially in production_, but the guard never inspects the
**config value**, which is the actual ingress that escapes normalization. A
future redeploy or a fresh environment with a mixed-case `seed_admin` walks
straight into the split.

**Recommendation.** Treat `seed_admin` as a third ingress point. Normalize it
once, at the boundary, with the same `normalize_email` helper — either in
`config::load_config` (canonicalize on load, so every consumer including the
dev-login `dev_email` injection sees the lowercased value) or at the two seed.rs
use sites _and_ the auth.rs:186 `dev_email` injection. Canonicalizing in config
is preferable: it is one site, it covers both the seed INSERT and the dev-login
round-trip, and it keeps the "two runtime ingress points" story honest. Add a
unit test: configure `seed_admin = "Admin@Corp.com"`, run the seed, drive a
dev-mode login, assert exactly one `users` row and that it carries the admin
role.

### C2 — "All lookups derive from the two ingress points" is false; sibling admin endpoints match raw operator emails, and `add_entity_role` fails silently

**Problem.** Design lines 113–115 state: "Everything downstream — token
creation, token/role lookups, and `builds.user_email` attribution — derives from
these two ingress points, so they remain mutually consistent without having to
touch every lookup site." This is the load-bearing justification for not doing a
`COLLATE NOCASE` migration. It is incorrect.

`builds.user_email` and the `AuthUser` lookups _are_ canonical (both derive from
the token's stored `payload.user`, verified at builds.rs:153/239 and
extractors.rs:252). But several admin endpoints take an email as a **fresh
path/body parameter supplied by the operator** and match it verbatim against the
stored (lowercased) identity, passing through **neither** ingress point:

- `revoke_all_tokens` — `get_user(&pool, &body.user_email)` (auth.rs:518)
- `deactivate_entity`, `activate_entity`, `set_entity_default_channel`,
  `get_entity_roles`, `replace_entity_roles`, `add_entity_role`,
  `remove_entity_role` — all `Path(email)`, matched verbatim (admin.rs).

This breaks the design's _own_ recommended workflow. The idempotency section
(lines 158–163) says: to adjust an existing user's roles, use
`PUT/POST /api/admin/entity/{email}/roles`. But if an admin provisions
`Alice@Corp.com` (stored lowercase `alice@corp.com`, because provisioning
normalizes) and then runs `POST /api/admin/entity/Alice@Corp.com/roles`, the
lookup mismatches.

The failure mode differs per endpoint and the worst one is silent:

- `replace_entity_roles` (admin.rs:1055): `set_user_roles` does
  `DELETE … WHERE user_email = ?` then `INSERT`. A mis-cased email deletes
  nothing and INSERTs role rows whose FK to `users(email)` does not resolve →
  `set_user_roles` is a plain transaction (not `OR IGNORE`), so this surfaces a
  **500** (FK violation). Loud, at least.
- `add_entity_role` (admin.rs:1211): `add_user_role` uses
  `INSERT OR IGNORE INTO user_roles …` (roles.rs:225). Under
  `PRAGMA foreign_keys = ON`, the FK violation on a non-existent `user_email` is
  suppressed by `OR IGNORE`. The handler then returns **`201 Created`** with the
  role echoed in the body (admin.rs:1237) — **a silent success that assigned
  nothing.** The admin believes the role is granted; it is not.

**Impact.** On a mixed-case deployment, or the moment any future row is
non-lowercase, admin role management silently misfires or 500s. On today's
all-lowercase deployment the latent bug is masked, but the design asserts it has
been _eliminated_ — that assertion is what makes this critical: it will be cited
later as license not to normalize, and the silent-201 path is exactly the kind
of bug that ships unnoticed.

**Recommendation.** Pick one and state it in the design: (a) Normalize
operator-supplied emails in a shared place — e.g. a custom `Path`/body extractor
or a one-line `normalize_email` at the top of each admin handler that takes an
email — so the "consistency" claim becomes true; or (b) adopt `COLLATE NOCASE`
on `users.email` (the design's rejected alternative), which fixes _all_ call
sites at once including these and is robust against future un-normalized
ingress. If (a), retract the "without having to touch every lookup site"
sentence — you _do_ have to touch them, and the design should enumerate which.
Independently of normalization, fix the `add_entity_role` silent-success: it
should verify the target user exists (it already loads `is_robot` at
admin.rs:1183 via `get_user`, which returns `Option` — reject `None` with 404)
before returning 201.

## Significant Concerns 🟡

### S1 — The "domain check factored out of `oauth::validate_user_info`" does not exist yet

**Problem.** Design line 174–177 says provisioning validates the email domain
"reusing the domain check factored out of `oauth::validate_user_info`." No such
factored function exists. The domain logic is inline (oauth.rs:90–95) and is
structurally entangled with the `email_verified` gate (oauth.rs:87–89), which
has no analog for an admin-provisioned address (there is no Google assertion to
verify). `allow_any_google_account` and `allowed_domains` both live on
`state.config.oauth`.

**Impact.** Not a flaw, but the design presents this as reuse when it is
actually a prerequisite refactor. If implemented naively by calling
`validate_user_info` with a synthetic `email_verified = true`, the provisioning
path inherits the `EmailNotVerified`-first ordering (an audit-rem D2 property
meaningful only for the OAuth flow) and the `GoogleUserInfo` wrapper, which is
awkward.

**Recommendation.** Extract a standalone
`fn domain_allowed(email: &str, allowed_domains: &[String], allow_any: bool) -> bool`
and have both `validate_user_info` and `provision_user`'s caller use it. State
in the design that this extraction is part of the work, not a pre-existing seam.

### S2 — Reject-on-duplicate is correct for active rows but wrong for a deactivated human; behavior is unspecified

**Problem.** The design maps "existing `is_robot = 0` row → `AlreadyExists`
(409)" unconditionally (lines 155–156), explicitly contrasting with the robot
`create_or_revive` which revives tombstoned rows. But humans _can_ be
deactivated (`deactivate_entity`, admin.rs:75 sets `active = 0`). A deactivated
human row therefore yields `AlreadyExists` on provision, and the design offers
no path to re-provision. The only recovery is `activate_entity` (which exists) —
fine — but the design does not say so, and a reviewer/operator reading "reject
duplicate" will reasonably expect provisioning to be the create-or-fix
primitive.

**Impact.** Operator confusion and a likely support escalation ("I provisioned
the user, it says AlreadyExists, but they can't log in"). Functionally
recoverable, but undocumented.

**Recommendation.** Keep reject-on-duplicate (it is the right call for humans —
unlike robots, a human identity is not a reissuable credential). But the design
should explicitly state: provisioning a deactivated human returns 409, and the
remediation is `PUT /api/admin/entity/{email}/activate` + the role endpoints.
Consider returning a 409 body that names the remediation. Note that this
interacts with C2: the `activate` path also takes a raw `Path(email)`.

### S3 — Robot-synthetic-email reject must run on the normalized value (ordering)

**Problem.** The design rejects `robot+…@robots` emails at provisioning
(line 170) and normalizes email (lowercase) at the same handler. The order
matters: `name_to_synthetic_email` and `validate_robot_name` (robots.rs:103–144)
permit uppercase in robot names, so a robot's synthetic email can be
`robot+CI@robots`. If an attacker/operator provisions `robot+CI@robots`,
`normalize_email` lowercases it to `robot+ci@robots`; `synthetic_email_to_name`
returns `Some("ci")`. If the reject check runs on the _raw_ input with a
case-sensitive `@robots` suffix match, a craft like `robot+ci@ROBOTS` could slip
past a naive check and then be lowercased into a colliding synthetic email.

**Impact.** Low (the `RobotCollision` branch and the `users` PK would still
catch an actual collision, and `@robots` is not a routable domain), but it is a
guard the design calls out, so it should be correct by construction.

**Recommendation.** Normalize first, then run all validation (robot prefix
reject, `robot:` name reject, domain check, role existence) on the normalized
value. State the ordering in the design's "Validation performed before the
write" list.

### S4 — `EntitySummary` / list query gains `first_login_at`; verify the `sqlx` offline cache + the all-NULL robot rendering

**Problem.** The design adds `first_login_at` to `EntitySummary` (users.rs:118)
and to the three `list_entities_filtered` queries (users.rs:147–175) plus
`EntityWithRolesItem` (admin.rs:897) and the `EntityRoleItem`/detail response.
Three points:

1. The three list queries are separate `sqlx::query!` invocations; adding a
   nullable column means re-running
   `cargo sqlx prepare --workspace -- --all-targets` (per cbsd-rs CLAUDE.md) and
   committing `.sqlx/`. The design's plan does not mention the offline-cache
   regeneration step, which is a frequent CI breakage.
2. `cbc admin users` (cbc/src/admin/users.rs) `UserWithRoles` (line 111) has no
   `first_login_at` field and the renderers (`cmd_list` line 206, `cmd_get`
   line 264) do not show it. The design says the CLI renders login state; this
   is net-new code, correctly scoped to commit 1, but the existing struct must
   gain a `#[serde(default)] first_login_at: Option<i64>` to stay
   forward-compatible.
3. The pending marker must be gated on `is_robot = 0` (design line 228, 310 —
   correct), because robots are always NULL. The renderer at cbc:206 prints a
   fixed column layout; ensure the pending logic does not mislabel a robot when
   `type=all`/`type=robot` is queried (the `users` subcommand always queries
   `type=user`, but the shared field leaks to robot views).

**Impact.** Build/CI breakage (offline cache) and a cosmetic mislabel risk for
robots. Both are caught by the design's own notes except the offline-cache step.

**Recommendation.** Add the `cargo sqlx prepare … -- --all-targets` step to the
plan explicitly. Add a `first_login_at: Option<i64>` to the cbc `UserWithRoles`.
Keep the `is_robot == 0` gate and unit-test the "robot is never pending"
rendering.

## Minor Observations 🟢

- **`ProvisionUserError` should reuse `is_unique_violation`.** The robot error
  enums implement `From<sqlx::Error>` that routes through
  `super::is_unique_violation` (robots.rs:89–97, db/mod.rs:65) to map `2067` →
  `UniqueViolation`. `provision_user` should do the same so the
  concurrent-create loser returns 409, not 500 — the design lists
  `UniqueViolation` but should explicitly state it reuses the shared helper
  rather than string-matching.
- **`ProvisionOutcome` with a single `Created` variant is odd.** A one-variant
  enum (design lines 126–128) adds no information over
  `Result<(), ProvisionUserError>`. It mirrors `CreateRevivedOutcome`
  structurally, but that enum has two variants for a reason (Created vs
  Revived). Either drop the enum or document that it exists for future extension
  (e.g. a later merge-on-conflict `Updated`).
- **Default-name local-part extraction.** The design defaults `name` to the
  email local-part (line 179). `email.split('@').next()` on a normalized email
  is safe, but specify behavior for a degenerate input with no `@` — domain
  validation (S1) should already reject it, so ordering (validate domain →
  derive name) makes this moot. Worth a one-line note.
- **`POST /api/admin/entities` vs `POST /api/admin/users`.** Either is fine.
  `entities` is the create verb on the collection `GET /api/admin/entities`
  lists, which is consistent; but the endpoint creates _only_ humans while the
  collection returns humans and robots, a slight asymmetry. Acceptable; the
  design already flags it as an open question.
- **Audit log field discipline.** The design says provisioning events are logged
  with email + roles + acting admin (line 343). Ensure the acting admin is
  `user.display_identity()` (the established pattern at admin.rs:148/173/262)
  for consistency, and that the email logged is the _normalized_ one.

## Strengths

- **Correct reuse of the robot concurrency pattern.** The `BEGIN IMMEDIATE`
  re-read-under-lock approach is exactly how `create_or_revive`
  (robots.rs:505–592) serializes concurrent creates so the loser gets a 409
  rather than a raw 500. Applying it verbatim to `provision_user` is the right
  call and respects the cbsd-rs pool-sizing invariant (`max_connections = 4`;
  one connection held for the transaction duration, same as robots — no new
  deadlock surface).
- **`first_login_at` backfill reasoning is sound.** `created_at` is a faithful
  first-login timestamp for login-created humans: `create_or_update_user`
  (users.rs:68–75) sets only `name`/`updated_at` on conflict and never rewrites
  `created_at`; only `revive_robot_in_conn` rewrites `created_at`
  (robots.rs:458–466) and that is `is_robot = 1`, excluded by the
  `WHERE is_robot = 0` backfill. The acknowledged seed-admin inaccuracy is
  correctly characterized.
- **Migration safety verified.** `is_robot` exists since migration 007, so
  `UPDATE users SET first_login_at = created_at WHERE is_robot = 0` in migration
  010 is valid. `ALTER TABLE ADD COLUMN` (nullable, no default) + a one-shot
  `UPDATE` runs exactly once under the embedded `sqlx::migrate!` runner
  (main.rs:126); there are no provisioned-NULL rows at migration time, so the
  backfill is well-defined and idempotent-by-construction (the migration version
  table prevents re-run). This part needs no further work.
- **Last-admin guard correctly assessed as not engaged.** Creation only ever
  adds a `users` row and at most adds a wildcard holder; it never reduces
  `count_active_wildcard_holders` (roles.rs:422–434). The guard is correctly out
  of scope.
- **The COLLATE NOCASE alternative is fairly evaluated.** The design's
  Alternatives section correctly identifies the FK-rebuild cost. (C2's
  recommendation reopens it only because the "ingress-only" claim turned out
  false.)

## Open Questions

1. **Where does `normalize_email` live, and does it cover `seed_admin`?** (C1)
   Is config-load canonicalization acceptable, or must the seed and dev-login
   sites be patched individually? Config-load is the smaller, safer surface.
2. **Does the design accept that operator-supplied emails on the sibling admin
   endpoints must be normalized too** (C2), or does it switch to
   `COLLATE NOCASE`? The "two ingress points" framing cannot stand unmodified
   either way.
3. **Is the deactivated-human re-provision path** (`409` → `activate` + roles)
   the intended operator workflow (S2)? If so, document it; if a future
   merge-on-conflict is planned, say so.
4. **Will the plan include the `cargo sqlx prepare … -- --all-targets`
   regeneration and `.sqlx/` commit** for the list-query column add (S4)?
5. **Does the audit log capture the acting admin via `display_identity()` and
   the normalized target email** (Minor)?

## Confidence Scoring

Scoring the **design** for readiness-to-implement. Deductions reflect
specified-but-broken behavior, missing prerequisites, and under-specification
that would mislead an implementer — not code defects (no code is written yet).

| Item                                                              | Points | Description                                                                                                |
| ----------------------------------------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------- |
| Starting score                                                    | 100    |                                                                                                            |
| D7: `seed_admin` not normalized → forked zero-cap admin (C1)      | -20    | Security/availability gap: dev-login pipes raw config email into the lowercasing callback; guard misses it |
| D7: silent `201` on `add_entity_role` with mismatched email (C2)  | -20    | `INSERT OR IGNORE` swallows the FK violation; admin role-grant silently no-ops; no existence check         |
| D8: "all lookups derive from two ingress points" is false (C2)    | -5     | Load-bearing design claim contradicted by 8 verbatim-match admin endpoints                                 |
| D1: domain-check refactor presented as pre-existing reuse (S1)    | -5     | The "factored-out" helper does not exist; required work omitted from the plan                              |
| D1: deactivated-human re-provision path unspecified (S2)          | -5     | Reject-on-duplicate has no documented remediation for a disabled human                                     |
| D8: robot-synthetic-email reject ordering vs normalization (S3)   | -5     | Validation-before-normalization ordering left implicit                                                     |
| D1: `sqlx` offline-cache regeneration step omitted from plan (S4) | -5     | Frequent CI breakage; cbsd-rs mandates `prepare … -- --all-targets` after query change                     |
| D11: cbc `UserWithRoles` forward-compat field not called out (S4) | -3     | Existing struct needs `#[serde(default)] first_login_at` to render login state                             |
| **Total**                                                         | **62** |                                                                                                            |

**Interpretation:** 62 / 100 — _Significant issues. Must address before
proceeding._ The design is structurally close to correct and the hard parts
(concurrency, backfill, migration) are right. But two concrete findings (C1, C2)
contradict the design's own claims and would ship latent admin-lockout /
silent-failure footguns the moment any non-lowercase email enters the system.
They are cheap to fix — both reduce to "normalize at the right boundary and
verify existence before echoing success" — but they must be fixed in the design
before implementation, because the v2 narrative actively argues _against_ doing
so.

## Verdict

**Revise and re-review.** Resolve C1 (normalize `seed_admin` / dev-login
ingress) and C2 (normalize operator-supplied admin emails _or_ adopt
`COLLATE NOCASE`, and fix the `add_entity_role` silent-success), then fold in
S1–S4. Re-submit as v2 of the design (or this review's v2). The underlying
approach is approvable; the framing and two boundary placements are not yet.
