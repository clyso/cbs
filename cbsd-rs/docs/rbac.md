# RBAC — Roles, Capabilities, and Scopes

This document describes the role-based access control system in
`cbsd-rs`: how it is modeled, what capabilities exist, how scopes
restrict access, and how to manage it through the `cbc` CLI. It
also covers divergences from the original Python `cbsd` and
identifies gaps requiring work for feature parity.

---

## Model Overview

Access control is hierarchical:

```
User → Role assignments
         └── Role definition
               ├── Capabilities (what you can do)
               └── Scopes (where you can do it)
```

- A **user** has zero or more **role assignments**.
- Each **role** defines both **capabilities** and **scopes**.
- Some capabilities enforce **scope** checks: the user must
  hold a role whose scopes satisfy the request.
- The wildcard capability `*` bypasses all scope checks and
  implies every capability.
- Assigning a role to a user grants all of that role's
  capabilities within all of that role's scopes.

### Divergence from Python cbsd

The Python `cbsd` uses a **static YAML file**
(`permissions.yaml`) with regex-based patterns, user-email regex
matching, and a two-layer capability system (route caps +
authorization caps). Changes require a server restart.

`cbsd-rs` replaces this with a **dynamic, database-backed RBAC
model**: roles and assignments are managed via REST API, patterns
use globs (not regex), and there is no separate route-level
capability layer. The capability enum is flat — no negative
capabilities (`-builds:revoke:any`), no regex expansion.

| Aspect | Python cbsd | cbsd-rs |
|--------|-------------|---------|
| Storage | `permissions.yaml` (static) | SQLite tables (dynamic) |
| User matching | Regex on email (`^.*@clyso\.com$`) | Explicit per-user role assignments |
| Patterns | Regex | Glob |
| Capability layers | Two (route caps + auth caps) | One (flat capability enum) |
| Negative caps | Supported (`-builds:revoke:any`) | Not supported |
| Management | Edit YAML, restart server | REST API + `cbc` CLI |
| Groups | Named groups in YAML | Named roles in database |
| Scopes | Per-group regex patterns | Per-role glob patterns |

---

## Built-in Roles

Three roles are seeded at first startup and cannot be modified
or deleted.

| Role | Capabilities | Scopes | Description |
|------|-------------|--------|-------------|
| `admin` | `*` | (none — global) | Full access |
| `builder` | `builds:create`, `builds:revoke:own`, `builds:list:own`, `builds:list:any`, `apikeys:create:own`, `workers:view`, `channels:view` | `channel=*`, `repository=*` | Create and manage own builds |
| `viewer` | `builds:list:any`, `workers:view` | (none — no scope-dependent caps) | Read-only access |

Custom roles can be created with any subset of the supported
capabilities and any combination of scopes.

---

## Capabilities

### Enforcement status

Each capability is listed with its actual enforcement status in
the current implementation. Capabilities marked **not enforced**
are defined in the `KNOWN_CAPS` validation list (new roles can
include them) but no route handler checks for them.

| Capability | What it grants | Enforced? | Notes |
|------------|---------------|-----------|-------|
| `builds:create` | Submit new builds | **Yes** | Scope-checked (channel, registry, repository) |
| `builds:list:own` | List own builds | **Yes** | Server filters to caller's email |
| `builds:list:any` | List any user's builds | **Yes** | Accepts `?user=` filter |
| `builds:revoke:own` | Cancel own builds | **Yes** | Ownership check in handler |
| `builds:revoke:any` | Cancel any user's builds | **Yes** | No ownership check |
| `apikeys:create:own` | Create API keys for self | **No** | Defined but never checked; any authenticated user can create API keys |
| `workers:view` | View worker list and status | **Yes** | |
| `workers:manage` | Register, deregister, rotate worker tokens | **Yes** | |
| `channels:view` | View channels and types | **Yes** | Implicit in `user_can_view_channel()` |
| `channels:manage` | Create, update, delete channels/types | **Yes** | |
| `periodic:create` | Create scheduled builds | **Yes** | Also requires `builds:create` |
| `periodic:view` | View periodic tasks | **Yes** | |
| `periodic:manage` | Update and delete periodic tasks | **Yes** | |
| `permissions:view` | View roles, users, assignments | **Yes** | |
| `permissions:manage` | Create/update/delete roles; assign/revoke user roles | **Yes** | |
| `components:manage` | Manage component definitions | **No** | Defined but never checked; `GET /api/components` requires only authentication |
| `admin:queue:view` | View build queue status | **Yes** | |
| `*` | Wildcard — all capabilities | **Yes** | Bypasses all scope checks |

### Unenforced endpoint gaps

These endpoints lack the capability checks they should have:

| Endpoint | Current state | Required fix |
|----------|--------------|--------------|
| `POST /api/auth/tokens/revoke-all` | Any authenticated user can revoke any user's tokens | Must check `permissions:manage` |
| `POST /api/auth/api-keys` | Any authenticated user can create keys | Should check `apikeys:create:own` |
| `GET /api/auth/api-keys` | Any authenticated user can list own keys | Should check `apikeys:create:own` |
| `DELETE /api/auth/api-keys/{prefix}` | Any authenticated user can revoke own keys | Should check `apikeys:create:own` |
| `GET /api/builds/{id}/logs/*` | Any authenticated user can read any build's logs | Should check `builds:list:own` or `builds:list:any` with ownership logic |
| `GET /api/components` | Any authenticated user can list components | Acceptable (matches Python), but `components:manage` has no enforcement point for mutations |

The `tokens/revoke-all` gap is a security issue: the design
document specifies `permissions:manage` is required, and a TODO
comment in the code says "Full permission check
(permissions:manage) is added in Commit 5" — but it was never
actually added.

---

## Scopes

Scopes restrict where a role's capabilities apply. They are
defined on the **role**, not per user assignment — all users
with the same role share the same scopes.

| Scope type | Format | Purpose | Enforcement |
|------------|--------|---------|-------------|
| `channel` | `<channel>/<type>` or glob | Restrict build submission | **Enforced** at `POST /api/builds` |
| `registry` | hostname glob | Restrict destination registry | **Enforced** at `POST /api/builds` (extracted from `dst_image.name`) |
| `repository` | exact name or glob | Restrict source repositories | **Enforced** at `POST /api/builds` (checked per component `repo` override) |

All three scope types are enforced at build submission. The
design document and the database schema support them; the route
handler collects all three and passes them to
`require_scopes_all()`.

### Channel scope patterns

| Pattern | Matches |
|---------|---------|
| `ces/dev` | Exactly `ces` channel, `dev` type |
| `ces/*` | Any type within `ces` |
| `*/dev` | `dev` type in any channel |
| `*` | All channels and types |

A pattern must contain `/` to be valid (it encodes
`channel/type`), except for the literal `*`.

### How scope checks work

At build submission the server checks that the user holds **at
least one role** whose scopes satisfy **all** scope requirements
of the request. Multiple scope types must be satisfied by a
**single role** (AND semantics) — the system does not combine
scopes across different roles.

### Divergence from Python cbsd

The Python system uses **regex patterns** per authorization
group. Each group defines independent patterns for project,
registry, and repository. There is no single-assignment AND
constraint — each scope type is checked independently.

`cbsd-rs` uses **glob patterns** per role with single-role AND
semantics. This is stricter: a user holding one role covering
channel `ces-devel/*` and a separate role covering registry
`harbor.clyso.com/ces-prod/*` cannot combine them to build for
`ces-devel` pushing to `ces-prod`.

---

## cbc CLI Reference

### Roles

```
cbc admin roles list
cbc admin roles get NAME
cbc admin roles create NAME --cap CAP [--cap CAP ...]
    [--scope TYPE=PATTERN ...] [--description DESC]
cbc admin roles update NAME --cap CAP [--cap CAP ...]
    [--scope TYPE=PATTERN ...]
cbc admin roles delete NAME [--force]
```

| Command | Notes |
|---------|-------|
| `list` | Prints all roles |
| `get` | Shows name, description, builtin flag, capabilities, scopes |
| `create` | At least one `--cap` required; `--scope` defines where the caps apply |
| `update` | Replaces the entire capability and scope set |
| `delete` | Fails if role has assignments unless `--force` |

### Users

```
cbc admin users list
cbc admin users get EMAIL
cbc admin users activate EMAIL
cbc admin users deactivate EMAIL
cbc admin users roles set EMAIL --role NAME [--role NAME ...]
cbc admin users roles add EMAIL --role NAME
cbc admin users roles remove EMAIL --role NAME
```

| Command | Notes |
|---------|-------|
| `list` | All users with assigned roles |
| `get` | Email, name, active, roles+scopes, effective caps |
| `activate` | Idempotent; re-enables account |
| `deactivate` | Revokes tokens + API keys; blocked for last admin |
| `roles set` | Replaces all role assignments (flat role names) |
| `roles add` | Adds one role |
| `roles remove` | Removes one role; blocked for last admin |

Scopes are defined on roles, not at assignment time. To
give a user different scopes, assign them a role that
carries those scopes.

### Channels and Types

Channels are image-destination mappings. Administering them
requires the `channels:manage` capability.

```
cbc admin channel create NAME [--description DESC]
cbc admin channel list
cbc admin channel delete NAME

cbc admin channel type-add CHAN_ID TYPE_NAME PROJECT
cbc admin channel type-update CHAN_ID TYPE_ID
    [--project P] [--prefix-template T]
cbc admin channel type-delete CHAN_ID TYPE_ID
cbc admin channel type-default CHAN_ID TYPE_ID

cbc admin user-set-default-channel EMAIL CHANNEL_ID
```

### Queue

```
cbc admin queue
```

Shows pending build counts per priority lane. Requires
`admin:queue:view`.

### Divergence from Python cbsd

The Python `cbc` has no admin commands for managing permissions.
The only auth-related commands are:

```
cbc auth login URL
cbc auth whoami
cbc auth perms list    # lists own capabilities
```

Permission changes in the Python system require editing
`permissions.yaml` and restarting the server. `cbsd-rs` provides
full CRUD for roles, user assignments, and scopes via the CLI.
This is a significant improvement.

---

## Guards and Invariants

| Guard | Condition | HTTP |
|-------|-----------|------|
| Last-admin | ≥1 active user must hold `*` | 409 |
| Built-in role | `admin`/`builder`/`viewer` immutable | 409 |
| Assignment guard | Delete role with assignments needs `--force` | 409 |
| Deactivation block | Cannot deactivate last admin | 409 |

The last-admin guard is enforced on 5 mutation paths:

1. `PUT /permissions/users/{email}/roles` (replace all)
2. `DELETE /permissions/users/{email}/roles/{role}` (remove one)
3. `PUT /admin/users/{email}/deactivate`
4. `DELETE /permissions/roles/{name}` (delete role)
5. `PUT /permissions/roles/{name}` (update capabilities)

### Divergence from Python cbsd

The Python system has **no last-admin guard** and **no
activation/deactivation concept**. Users are created
automatically on first OAuth login and cannot be disabled
without editing the YAML file. There is no concept of built-in
roles — groups are defined in YAML and can be freely edited.

---

## Authentication Methods

Both methods produce the same capability set for a user.

| Method | Source | Use case |
|--------|--------|----------|
| PASETO token | Issued after Google OAuth | Human users, CLI |
| API key | `cbsk_` prefix; created via CLI | Services, CI/CD |

Deactivated users are rejected with HTTP 401 regardless of
token validity.

### Divergence from Python cbsd

| Aspect | Python cbsd | cbsd-rs |
|--------|-------------|---------|
| Token TTL | Configurable via `token_secret_ttl_minutes` | Configurable via `max_token_ttl_seconds`; default 6 months |
| Token revocation | None — tokens valid until TTL expires | Immediate via DB check on every request |
| API keys | Not supported | Full support with argon2 hashing, LRU cache |
| User creation | Automatic on first OAuth login | Automatic on first OAuth login + seed admin |
| User deactivation | Not supported | Via REST API, revokes all credentials |

---

## Effective Capabilities

A user's effective capabilities are the **union** of all
capabilities across all assigned roles. The wildcard `*`
short-circuits: if any role contains `*`, the user has full
access.

`cbc admin users get EMAIL` shows both individual role
assignments and the computed effective capability set.

---

## Parity Assessment: Python cbsd vs cbsd-rs

### Features fully implemented in cbsd-rs

- Dynamic role management (CRUD via REST API and CLI)
- Role-level scoped capabilities with user assignment
- `builds:create` with channel/registry/repository scoping
- `builds:list:own` / `builds:list:any` with ownership filtering
- `builds:revoke:own` / `builds:revoke:any` with ownership
  check
- `permissions:view` / `permissions:manage` for role + user
  administration
- `workers:view` / `workers:manage` for worker lifecycle
- `channels:view` / `channels:manage` for channel/type CRUD
- `periodic:create` / `periodic:view` / `periodic:manage` for
  scheduled builds
- `admin:queue:view` for build queue inspection
- Last-admin guard on all 5 mutation paths
- Built-in role protection
- User activation / deactivation with credential revocation
- API key support (Python has none)
- Token revocation (Python has none)
- Seed admin + seed worker API keys at first startup

### Features present in Python cbsd but NOT in cbsd-rs

- **Regex-based user matching.** Python matches users by email
  regex (`^.*@clyso\.com$`), allowing blanket rules for entire
  domains. `cbsd-rs` requires explicit per-user assignments.
  This is a deliberate design change (more auditable) but means
  new employees must be manually assigned roles.

- **Negative capabilities.** Python supports
  `[".*", "-builds:revoke:any"]` to grant all-except patterns.
  `cbsd-rs` has no negation — roles define only positive grants.

- **Capability regex expansion.** Python allows `builds:.*` to
  match all build capabilities. `cbsd-rs` requires listing each
  capability explicitly.

- **Group composition.** Python rules can reference multiple
  groups (`groups: [admin, releases, development]`), composing
  capabilities from multiple named sets. `cbsd-rs` achieves
  this via multiple role assignments per user, which is
  functionally equivalent.

### Gaps requiring implementation work

| Gap | Severity | Detail |
|-----|----------|--------|
| `tokens/revoke-all` missing permission check | **Critical** | Any authenticated user can revoke any user's tokens. Design requires `permissions:manage`. |
| `apikeys:create:own` never enforced | Medium | Defined in KNOWN_CAPS and assigned to `builder` role, but no route handler checks it. Any authenticated user can create/list/revoke own API keys. |
| `components:manage` never enforced | Low | Defined in KNOWN_CAPS but no route handler checks it. Only `GET /api/components` exists (read-only), and it requires only authentication — same as Python. No mutation endpoint exists for components. |
| Build log endpoints missing capability checks | Medium | `GET /api/builds/{id}/logs/*` (tail, follow, full) require authentication but no capability check. A viewer or builder with `:own` can read logs for builds they don't own by guessing the ID. |
| No `workers:manage` equivalent in Python | N/A | Python has no worker management API. This is a cbsd-rs addition, not a parity gap. The capability is correctly enforced. |

### Python capabilities not mapped to cbsd-rs

The Python system defines capabilities that have no direct
equivalent in `cbsd-rs`:

| Python capability | Status in cbsd-rs |
|-------------------|-------------------|
| `project:list` | Replaced by `channels:view` (channels replaced projects) |
| `project:manage` | Replaced by `channels:manage` |
| `routes:auth:login` | Removed — login is unauthenticated by design |
| `routes:auth:permissions` | Replaced by `permissions:view` |
| `routes:builds:new` | Replaced by `builds:create` |
| `routes:builds:revoke` | Replaced by `builds:revoke:own`/`:any` |
| `routes:builds:status` | Replaced by `builds:list:own`/`:any` |
| `routes:builds:inspect` | Not ported (Celery-specific; no equivalent in cbsd-rs) |
| `routes:periodic:*` | Replaced by `periodic:*` capabilities |

The separate route-level capability layer
(`ROUTES_AUTH_PERMISSIONS`, `ROUTES_BUILDS_NEW`, etc.) is
intentionally removed. In `cbsd-rs`, endpoint access is implied
by having the relevant capability — there is no additional
"can you access this route" check.

### Python endpoints with known permission issues

These issues exist in the Python `cbsd` and are resolved in
`cbsd-rs`:

| Python issue | cbsd-rs status |
|-------------|----------------|
| `GET /builds/status/{task_id}` — no authentication at all | Fixed: `GET /api/builds/{id}` requires auth + `builds:list:*` |
| `GET /builds/status?all=true` — no BUILDS_LIST_ANY check | Fixed: `builds:list:any` enforced, `:own` filtered |
| `DELETE /builds/revoke/{id}?force=true` — no cap distinction | Fixed: separate `builds:revoke:own` / `builds:revoke:any` |
| No token revocation mechanism | Fixed: `POST /api/auth/token/revoke` and bulk revocation |
| Debug logging leaks token plaintext | Fixed: tokens not logged |

---

## Route-to-Capability Map

Complete mapping of every authenticated endpoint to its
capability check.

### Build routes

| Route | Method | Capability | Scope |
|-------|--------|-----------|-------|
| `/api/builds` | POST | `builds:create` | channel + registry + repository |
| `/api/builds` | GET | `builds:list:own` OR `:any` | — |
| `/api/builds/{id}` | GET | `builds:list:own` OR `:any` | — |
| `/api/builds/{id}` | DELETE | `builds:revoke:own` OR `:any` | — |
| `/api/builds/{id}/logs/*` | GET | **None (gap)** | — |

### Periodic routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/periodic` | POST | `periodic:create` AND `builds:create` |
| `/api/periodic` | GET | `periodic:view` |
| `/api/periodic/{id}` | GET | `periodic:view` |
| `/api/periodic/{id}` | PUT | `periodic:manage`; +`builds:create` if updating descriptor |
| `/api/periodic/{id}` | DELETE | `periodic:manage` |
| `/api/periodic/{id}/trigger` | POST | `periodic:manage` |
| `/api/periodic/{id}/retry` | POST | `periodic:manage` |

### Permission routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/permissions/roles` | GET | `permissions:view` |
| `/api/permissions/roles` | POST | `permissions:manage` |
| `/api/permissions/roles/{name}` | GET | `permissions:view` |
| `/api/permissions/roles/{name}` | PUT | `permissions:manage` |
| `/api/permissions/roles/{name}` | DELETE | `permissions:manage` |
| `/api/permissions/users` | GET | `permissions:view` |
| `/api/permissions/users/{email}/roles` | GET | `permissions:view` |
| `/api/permissions/users/{email}/roles` | PUT | `permissions:manage` |
| `/api/permissions/users/{email}/roles` | POST | `permissions:manage` |
| `/api/permissions/users/{email}/roles/{role}` | DELETE | `permissions:manage` |

### Admin routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/admin/users/{email}/deactivate` | PUT | `permissions:manage` |
| `/api/admin/users/{email}/activate` | PUT | `permissions:manage` |
| `/api/admin/users/{email}/default-channel` | PUT | `permissions:manage` |
| `/api/admin/queue` | GET | `admin:queue:view` |
| `/api/admin/workers` | POST | `workers:manage` |
| `/api/admin/workers/{id}` | DELETE | `workers:manage` |
| `/api/admin/workers/{id}/regenerate-token` | POST | `workers:manage` |

### Channel routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/channels` | GET | Scope-based visibility (channels:view/manage or channel scope) |
| `/api/channels/{id}` | GET | Same as list |
| `/api/channels` | POST | `channels:manage` |
| `/api/channels/{id}` | PUT | `channels:manage` |
| `/api/channels/{id}` | DELETE | `channels:manage` |
| `/api/channels/{id}/types` | POST | `channels:manage` |
| `/api/channels/{id}/types/{tid}` | PUT | `channels:manage` |
| `/api/channels/{id}/types/{tid}` | DELETE | `channels:manage` |
| `/api/channels/{id}/default-type` | PUT | `channels:manage` |

### Worker routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/workers` | GET | `workers:view` |

### Auth routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/auth/login` | GET | None (OAuth initiation) |
| `/api/auth/callback` | GET | None (OAuth callback) |
| `/api/auth/whoami` | GET | Auth only |
| `/api/auth/token/revoke` | POST | Auth only (self-revoke) |
| `/api/auth/tokens/revoke-all` | POST | **None (gap — needs `permissions:manage`)** |
| `/api/auth/api-keys` | POST | **Auth only (gap — needs `apikeys:create:own`)** |
| `/api/auth/api-keys` | GET | **Auth only (gap — needs `apikeys:create:own`)** |
| `/api/auth/api-keys/{prefix}` | DELETE | **Auth only (gap — needs `apikeys:create:own`)** |

### Component routes

| Route | Method | Capability |
|-------|--------|-----------|
| `/api/components` | GET | Auth only |
