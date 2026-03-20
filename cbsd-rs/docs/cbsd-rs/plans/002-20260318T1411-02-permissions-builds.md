# Phase 3 — Permissions & Builds: RBAC, Queue, Submission, Listing

## Progress

| Item | Status |
|------|--------|
| Commit 5: RBAC system — roles, caps, scopes, last-admin guard | Done |
| Commit 6: Build queue, submission, listing, components | Done |

## Goal

RBAC permission system fully operational with all 5 last-admin guard paths.
Builds can be submitted, queued, listed, and revoked (QUEUED state only —
dispatch to workers comes in Phase 4).

## Depends on

Phase 2 (auth extractors needed for gated endpoints).

## Commit 5: RBAC system — roles, caps, scopes, last-admin guard

The full permission model.

**db/roles.rs:**
- CRUD for `roles`, `role_caps`, `user_roles`, `user_role_scopes` tables.
- `get_effective_caps(user_email)` — joins user_roles → role_caps, returns
  all capabilities.
- `get_user_assignments_with_scopes(user_email)` — returns role assignments
  with their per-assignment scopes for scope evaluation.
- Builtin role protection: cannot delete or modify caps of `builtin=1` roles
  (returns 409).

**auth/extractors.rs (extend):**
- `RequireCap<C>` extractor — loads effective caps via `AuthUser`, checks
  required capability. Returns 403 with missing cap detail.
- `:own`/`:any` helper: `user.has_any_cap(&[...])` for OR-capability
  endpoints. Handler-level ownership check pattern.
- `require_scopes_all(scope_checks)` — assignment-level AND semantics. Finds
  one assignment satisfying ALL scope checks. Scope types: `channel` (against
  `descriptor.channel`), `registry` (hostname from `dst_image.name`),
  `repository` (each `components[].repo` override).

**routes/permissions.rs:**
- All `/api/permissions/roles/*` and `/api/permissions/users/*` endpoints.
- Scope validation on assignment: rejects scope-gated roles without scopes
  (400). PUT replace-all rejects if scope-dependent roles would be left
  scopeless.
- Last-admin invariant checked on all 5 mutation paths: role assignment
  PUT/DELETE, user deactivation, role deletion (`?force=true` for CASCADE),
  role cap update. All return 409 if invariant violated.

**routes/admin.rs:**
- `PUT /api/admin/users/{email}/deactivate` — transactional: set active=0,
  bulk-revoke tokens + API keys, purge LRU cache entries. Idempotent (no-op
  if already inactive). Last-admin guard via transactional check.
- `PUT /api/admin/users/{email}/activate` — idempotent, does not restore
  revoked credentials.

**Testable:** Create roles, assign to users with scopes, verify capability
checks. Assignment-level AND scope evaluation — **explicit test case:** two
assignments each covering a different scope type → build submission rejected
with 403, even though both scope types are individually satisfied by
different assignments. Last-admin guard prevents removing sole admin — **5
named test cases:** (1) role assignment PUT, (2) role assignment DELETE,
(3) user deactivation, (4) role deletion, (5) role capability update.
Builtin role protection (cap modification returns 409). User
deactivation/activation (idempotent).

## Commit 6: Build queue, submission, listing, components

Builds can be submitted and queued, but not yet dispatched.

**queue/mod.rs:**
- `BuildQueue` with 3 `VecDeque<QueuedBuild>` lanes (high/normal/low).
  `SharedBuildQueue = Arc<tokio::sync::Mutex<BuildQueue>>`.
- `enqueue(build)` — pushes to appropriate priority lane.
- `next_pending()` — pops from highest non-empty lane.
- **Ownership boundary:** Phase 3 defines the queue lanes only. The `active`
  map (`HashMap<BuildId, ActiveBuild>`) and worker registry are added in
  Phase 4 Commit 7 when the WS handler is introduced. `ActiveBuild` is
  defined in Phase 4 (at minimum: `build_id`, `connection_id`,
  `dispatched_at`, `descriptor`). This split is intentional — the queue is
  testable without workers.

**db/builds.rs:**
- `insert_build(descriptor, user_email, priority)` — state=QUEUED.
- `update_build_state(id, new_state, ...)`.
- `get_build(id)`, `list_builds(filter)` — filterable by state, user_email.
- `insert_build_log_row(build_id, log_path)` — creates `build_logs` entry
  with `log_size=0`, `finished=0`. Called at dispatch time (Commit 8a).

**routes/builds.rs:**
- `POST /api/builds` — validates descriptor (component names against
  component store), checks `builds:create` cap + scope (channel + registry +
  repository via `require_scopes_all`), inserts to DB as QUEUED, enqueues in
  memory, server overwrites `signed_off_by` from `users` table. Returns 202
  with warning if no matching worker connected.
- `GET /api/builds` — `:own` filters to caller; `:any` allows `?user=`
  filter (403 if caller lacks `:any`).
- `GET /api/builds/{id}` — `:own` checks ownership, `:any` skips.
  Response shape per design doc (epoch timestamps, lowercase states).
- `DELETE /api/builds/{id}` — for QUEUED state: acquire `SharedBuildQueue`
  mutex → search lanes for build_id → if found, remove from lane + update DB
  to REVOKED under mutex → return 200. If not found in queue (race: already
  dispatched), fall through to DISPATCHED handling (deferred to Phase 4).
  Active states handled in Phase 4.

**routes/components.rs:**
- `GET /api/components` — lists component names + versions from filesystem.
  No capability required beyond authentication.

**components/mod.rs:**
- Filesystem scan of `components/` directory.
- Loads `cbs.component.yaml` descriptors.
- Validates component names at build submission time (unknown → 400).

**Log endpoints for QUEUED builds:** `GET /builds/{id}/logs/tail` and
`/logs/follow` return 404 with `{"detail": "no logs yet"}` when no
`build_logs` row exists (QUEUED builds have no logs until dispatched).

**routes/admin.rs (extend):**
- `GET /api/admin/queue` — queue state (pending counts per lane, active
  builds). Requires `admin:queue:view`.

**Testable:** Submit builds, verify queued. List builds with own/any
filtering. Revoke QUEUED builds. Component listing. Queue state inspection.
Scope checks on build submission (channel + registry + repository
assignment-level AND).
