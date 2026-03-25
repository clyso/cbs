# cbsd Rust Port — Authentication, Permissions & User-Facing API

## Overview

This document describes the authentication, authorization, and user-facing API
design for the Rust reimplementation of `cbsd`. The goals are:

- Google SSO as the sole authentication method for human users
- No password storage
- Dynamic permission management via REST API (replacing static YAML)
- SQLite-backed user and permission storage
- Support for both CLI (`cbc`) and future web UI clients

## Authentication

### Principles

- **No passwords.** The server never stores or manages passwords. Google SSO
  handles identity, MFA, password policies, and account recovery.
- **Two authentication methods:**
  - **Human users:** Google OAuth 2.0 (OIDC) → PASETO v4 token
  - **Service accounts:** Admin-provisioned API key (long-lived, stored in DB)
- Both methods produce the same internal identity for downstream authorization.
  Route handlers do not distinguish how a caller authenticated.

### PASETO Tokens

Tokens use PASETO v4 (local/symmetric encryption), same as the current Python
implementation. Wire format is interoperable.

- **Default TTL: infinite (0).** Users of a build tool authenticate rarely; a
  short TTL creates friction without meaningful security benefit.
- **Configurable per-token.** Admin or user can request a specific TTL at
  creation time.
- **Revocation:** The server checks token validity against the database on
  every request, not just the cryptographic signature. This means a revoked
  token is rejected immediately, at the cost of one DB lookup per request
  (negligible for this load).
- **Token hash function:** PASETO tokens are hashed with **SHA-256** for the
  `tokens.token_hash` column. Argon2 is **not** used for token hashes —
  PASETO tokens are already cryptographically protected by symmetric
  encryption, and argon2's deliberate slowness (100–500ms) would make every
  authenticated API request unacceptably slow. Argon2 is reserved for API key
  hashes only (see below).
- **`max_token_ttl_seconds` server config:** Enforces an upper bound on token
  TTL even if the client requests infinite. Default: `none` (no limit —
  infinite TTL allowed). When set to a positive integer, tokens with TTL
  exceeding the limit are clamped to the limit. Lets operators tighten policy
  without code changes.

### Service Account API Keys

For CI pipelines, automation, and worker registration — any non-interactive
client that cannot complete a browser-based OAuth flow.

- Admin creates an API key via the REST API (`permissions:manage`).
  Users can also create their own API keys (`apikeys:create:own`).
- Key is a random opaque string (e.g., `cbsk_<32 random bytes as hex>`).
  The `key_prefix` stores the first **12 characters of the random portion**
  (post-`cbsk_` prefix), giving 48 bits of prefix space for identification.
- Stored hashed (**argon2**) in the DB. Argon2 provides offline brute-force
  resistance. The plaintext is shown once at creation and never again.
- **LRU verification cache:** To avoid paying argon2 (100–500ms) on every
  HTTP request from CI pipelines or `cbc` using API keys, the server maintains
  an in-memory cache with reverse indices:

  ```rust
  // Shared as Arc<tokio::sync::Mutex<ApiKeyCache>> in AppState.
  // The mutex must be held across all multi-map operations atomically.
  struct CachedApiKey {
      owner_email: String,
      key_prefix: String,   // needed for reverse-map cleanup on LRU eviction
      roles: Vec<RoleAssignment>,
      expires_at: Option<i64>,
  }

  struct ApiKeyCache {
      by_sha256: LruCache<[u8; 32], CachedApiKey>,  // primary lookup (cap 512)
      by_prefix: HashMap<String, [u8; 32]>,          // prefix → sha256
      by_owner: HashMap<String, HashSet<[u8; 32]>>,  // email → set of sha256
  }
  ```

  **Concrete eviction pattern** (the `lru` crate 0.12 has no `on_evict`
  callback; eviction happens implicitly via `push()` which returns the
  evicted entry):

  ```rust
  fn insert(&mut self, sha256: [u8; 32], entry: CachedApiKey) {
      if let Some((evicted_sha, evicted)) = self.by_sha256.push(sha256, entry.clone()) {
          // Clean up reverse maps for evicted entry
          self.by_prefix.remove(&evicted.key_prefix);
          if let Some(set) = self.by_owner.get_mut(&evicted.owner_email) {
              set.remove(&evicted_sha);
              if set.is_empty() { self.by_owner.remove(&evicted.owner_email); }
          }
      }
      self.by_prefix.insert(entry.key_prefix.clone(), sha256);
      self.by_owner.entry(entry.owner_email.clone()).or_default().insert(sha256);
  }
  ```

  - **Lookup:** Acquire mutex. SHA-256 of raw API key → `CachedApiKey`.
  - **Individual revocation** (`DELETE /auth/api-keys/{prefix}`): acquire
    mutex. Use `by_prefix` to find SHA-256, pop from `by_sha256`, clean up
    `by_owner`.
  - **Bulk deactivation** (`PUT /admin/users/{email}/deactivate`): acquire
    mutex. Drain `by_owner[email]`, pop each from `by_sha256`, clean up
    `by_prefix`.
- API keys are sent as `Authorization: Bearer <key>`, same header as PASETO
  tokens. The server distinguishes them by prefix (`cbsk_` vs PASETO format).
- API keys can have an optional TTL or be infinite.
- Admin can list, revoke, and rotate any API key. Users can list and revoke
  their own.

### Domain restriction

The server restricts which Google accounts can authenticate via
`allowed_domains` in the server config:

```yaml
allowed_domains:
  - clyso.com
```

**Enforcement:**

1. The Google authorization URL includes `hd=<first_domain>` as a hint (shows
   only matching accounts in the Google picker). This is defense-in-depth only.
2. The **server-side check** is the real gate: at callback, after exchanging
   the authorization code and extracting the email, the server verifies the
   email domain against `allowed_domains`. If it doesn't match, the server
   returns HTTP 403 without creating a user record or issuing a token.

If `allowed_domains` is empty or absent, the server refuses to start unless
`allow_any_google_account: true` is explicitly set. This prevents accidental
open access.

## OAuth Flow

### Session state for OAuth

OAuth requires server-side session state to store the CSRF `state` parameter
between the login redirect and the callback. This is handled by
`tower-sessions` with a **SQLite-backed session store**
(`tower-sessions-sqlx-store`). Sessions are ephemeral and only used during the
OAuth flow — they are not used for ongoing API authentication.

**Why SQLite, not in-memory:** An in-memory store loses all in-flight OAuth
sessions on server restart, causing CSRF validation failures for any user
mid-flow. SQLite sessions survive restarts at negligible cost (sessions are
short-lived and rare).

**Session signing key:** Derived from the `token_secret_key` in server config
via HKDF-SHA256 with context string `cbsd-oauth-session-v1`. This produces a
distinct key from the PASETO signing key despite sharing the same input
material. The derivation is deterministic — sessions survive server restarts
as long as `token_secret_key` doesn't change.

**Stored in session at `/login` time:** `oauth_state` (CSRF nonce),
`client_type` (`cli` or `web`), and optionally `cli_port` (for localhost
auto-redirect). The `client_type` is read at `/callback` time to determine
the response format — Google's callback only carries `code` and `state`, so
the client type must survive the round-trip via the session.

**Session fixation prevention:** At callback, after validating the `state`
parameter, the server **regenerates the session ID** before issuing the PASETO
token. This prevents an attacker who set the session cookie before the OAuth
flow from receiving the victim's token. `tower-sessions` supports session ID
regeneration via `session.cycle_id()`.

**OAuth session TTL:** Sessions used for the OAuth flow have a short TTL
(10 minutes). An incomplete flow (user starts login but never completes the
Google round-trip) leaves an orphaned session row that is automatically
cleaned up after expiry.

### Shared flow (CLI and Web UI)

```
1. Client directs user to: GET /api/auth/login?client=cli|web
2. Server generates OAuth state, stores in session, redirects to Google
3. User authenticates with Google
4. Google redirects to: GET /api/auth/callback?code=...&state=...
5. Server exchanges code for ID token, extracts email + name
6. Server creates or updates user record in DB
7. Server creates PASETO token for the user
8. Response depends on client type (see below)
```

### CLI flow

When `?client=cli`:

**MVP:** The callback renders an HTML page displaying the token as a copyable
string. The user pastes it into the CLI tool (e.g., `cbc login` prompts for
the token).

```
$ cbc login
Opening browser for authentication...
Paste your token: cbst_v4.local.xxxxxxxxx...
Authenticated as joao.luis@clyso.com
Token saved to ~/.config/cbc/config.json
```

**v1 enhancement: localhost auto-redirect.** The CLI starts a temporary HTTP
server on `localhost:<port>` and encodes the port in the login URL as
`?client=cli&cli_port=<port>`. The OAuth round-trip happens entirely against
the server's registered redirect URI (the server's own callback URL — Google
never sees `localhost`). At callback, the server detects `cli_port` in the
session and renders a page with a JavaScript redirect to
`http://localhost:<port>/callback?data=<base64>`. The CLI receives the token
automatically, no manual paste needed.

**Implementation constraint:** Google requires all redirect URIs registered
in advance. The registered URI is always the server's own callback URL. The
localhost redirect is a client-side hop (JavaScript/meta-refresh), not a
Google redirect. This is the same pattern used by `gcloud auth login` and
`gh auth login`.

The paste-based flow remains available as a fallback for headless environments
(when `cli_port` is absent, the callback page displays the base64 string for
manual copy).

**Security:** The token-display HTML page includes
`Content-Security-Policy: default-src 'none'; script-src 'none';` to prevent
exfiltration via injected scripts.

### Web UI flow

When `?client=web`:

The callback returns the PASETO token in the URL fragment
(`#token=<base64>`) and redirects to the web UI root (`/`). The web UI
extracts the token from the fragment, stores it in `localStorage`, and sends
it as `Authorization: Bearer <token>` on all API requests — the same
mechanism as the CLI. This avoids introducing a second auth path (session
cookies + CSRF protection) for the web UI.

The `tower-sessions` session cookie is used **only** during the OAuth
round-trip (storing `oauth_state` and `client_type`). It is not used for
ongoing API authentication.

### Client type detection

The `?client=cli|web` query parameter on `/api/auth/login` determines the
callback behavior. If omitted, defaults to `cli` for backwards compatibility.

## Endpoint Gating

### Approach: axum custom extractors

The idiomatic axum pattern for auth + permission checks is custom extractors
that implement `FromRequestParts`. No external framework or middleware needed.

```rust
// Extractor: any authenticated user (token or API key)
async fn whoami(user: AuthUser) -> Json<UserInfo> { ... }

// Extractor: authenticated + must have a specific capability
async fn create_build(
    user: RequireCap<{ Cap::BuildsCreate }>,
    Json(body): Json<NewBuildRequest>,
) -> Result<Json<NewBuildResponse>, AppError> { ... }
```

The `AuthUser` extractor:

1. Reads `Authorization: Bearer <token>` header (via `axum-extra`
   `TypedHeader`).
2. Identifies token type by prefix (PASETO vs API key).
3. Validates the token (crypto check + DB lookup for revocation).
4. Returns the user record with loaded roles and capabilities.
5. On failure: returns 401 Unauthorized.

The `RequireCap<C>` extractor:

1. Delegates to `AuthUser` for authentication.
2. Checks that the user's roles grant capability `C` (optionally scoped to the
   request's project/registry/repository).
3. On failure: returns 403 Forbidden with a message indicating the missing
   capability.

This is equivalent to the current Python `RequiredRouteCaps` FastAPI
dependency, expressed as Rust types with compile-time capability constants.

### Scoped permission checks

Some capabilities are scoped to a channel, registry, or repository. For
example, a user may have `builds:create` only for channel `ces-devel/*`. The
scope check happens inside the route handler after extracting the request body.

**Scope type field mapping:**

| Scope type | Checked against | Source in BuildDescriptor |
|------------|----------------|--------------------------|
| `channel` | Build channel | `descriptor.channel` (e.g., `ces-devel`, `ces-prod`) |
| `registry` | Destination image registry hostname | Extracted from `descriptor.dst_image.name` (e.g., `harbor.clyso.com` from `harbor.clyso.com/ces-devel/ceph:v19`) |
| `repository` | Allowlist of source repos the user can pull from | `descriptor.components[].repo` — only checked for components with a `repo` override. If a component has no `repo` override (uses the component's default), no repository scope check is needed for that component. |

**Assignment-level AND semantics:** All scope checks for a single build
submission must be satisfied by the **same assignment**. Independent per-type
checks are NOT used — this prevents the confused-deputy problem where
different assignments satisfy different scope types, authorizing combinations
no single assignment permits.

Example of what is **prevented**: Alice has assignment A (`channel=ces-devel/*`)
and assignment B (`registry=harbor.clyso.com/ces-prod/*`). A build targeting
`channel=ces-devel` pushing to `harbor.clyso.com/ces-prod` is **rejected** —
no single assignment authorizes both the channel and the registry.

**Handler implementation:**

```rust
async fn create_build(
    user: RequireCap<{ Cap::BuildsCreate }>,
    state: State<AppState>,
    Json(body): Json<NewBuildRequest>,
) -> Result<Json<NewBuildResponse>, AppError> {
    let desc = &body.descriptor;

    // Collect all scope checks for this build
    let mut scope_checks = vec![
        (ScopeType::Channel, desc.channel.clone()),
    ];

    // Registry: extract hostname from dst_image.name
    if let Some(registry_host) = extract_registry_host(&desc.dst_image.name) {
        scope_checks.push((ScopeType::Registry, registry_host));
    }

    // Repository: check each component with a repo override
    for comp in &desc.components {
        if let Some(repo) = &comp.repo {
            scope_checks.push((ScopeType::Repository, repo.clone()));
        }
    }

    // All checks must be satisfied by a SINGLE assignment (AND semantics)
    user.require_scopes_all(&scope_checks)?;

    // ... proceed with build
}
```

`require_scopes_all` iterates the user's assignments that grant `builds:create`
and returns Ok if **any one assignment** satisfies **all** scope checks. If no
single assignment covers all checks, it returns 403.

**Multi-role evaluation:** The user may have `builds:create` from multiple
role assignments (e.g., `builder` assigned twice with different scopes). Each
assignment is checked independently. The build is authorized if at least one
assignment passes all scope checks.

This two-phase check (capability at extractor level, scope at handler level)
keeps the extractor simple while allowing fine-grained access control where
needed.

## Permissions Model

### Current model (Python)

The current system uses a static `permissions.yaml` file with:

- **Groups** containing authorization rules, each with:
  - A type (`project`, `registry`, `repository`, `routes`)
  - A regex pattern
  - A list of capabilities (with negative caps prefixed by `-`)
- **Rules** mapping user email patterns (regex) to groups

Problems:

- Two redundant layers: route caps and authorization caps
- Regex patterns are error-prone (quadruple-escaped dots in YAML)
- Static file requires server restart for changes
- Negative capabilities are confusing
- No API for management

### New model

A single-layer RBAC (Role-Based Access Control) model with scoped capabilities.

#### Roles

A role is a named collection of capabilities with optional scopes.

```
Role:
  name: string                 // "admin", "builder", "viewer", or custom
  description: string
  capabilities: [Cap]          // what operations the role grants
  scopes: [Scope]              // what resources the caps apply to
```

Roles are stored in the database and managed via REST API.

#### Capabilities

A flat enum of operations. No hierarchy, no negation.

```
Cap:
  // Build operations
  builds:create
  builds:revoke:own
  builds:revoke:any
  builds:list:own
  builds:list:any
  admin:queue:view              // queue internals, admin-only, intentionally unscoped

  // Periodic build operations (deferred to post-v1)
  // periodic:create
  // periodic:manage
  // periodic:view

  // Administrative
  permissions:view
  permissions:manage          // can create/modify roles and user-role assignments
  apikeys:create:own          // can create and manage own API keys (self-service)
  components:manage           // can update component definitions
  workers:view                // can see connected workers and their status

  // Wildcard (admin only)
  *                           // all capabilities
```

No negative capabilities. Roles define what is allowed, not what is denied.
This is simpler to reason about and audit.

#### Scopes

A scope limits where a capability applies. Scopes use **glob patterns** (not
regex) for simplicity and readability.

```
Scope:
  type: "channel" | "registry" | "repository"
  pattern: string              // glob: "ces-devel/*", "harbor.clyso.com/ces/*", "*"
```

**Scopes live on assignments, not roles.** A role defines *what you can do*
(capabilities). The user-role assignment defines *where you can do it*
(scopes). This allows the same `builder` role to be assigned to different users
with different scopes, without creating separate roles per scope set.

- Roles with `*` capability (admin) need no scopes — they are global by
  definition. The scope check is skipped entirely.
- Scope-dependent capabilities (e.g., `builds:create`) require scopes at
  assignment time. The assignment API rejects attempts to assign such roles
  without scopes.

**Glob instead of regex:** The current YAML uses regex with quadruple-escaped
dots (`^harbor\\\\.clyso\\\\.com/c[ce]s-devel/.*$`). Glob patterns cover all
current use cases (`harbor.clyso.com/ces-devel/*`) and are much easier to
write and audit.

#### User-role assignments

Users are assigned one or more roles, each with optional scopes. A user's
effective capabilities are the union of all capabilities from all their roles.
Scope checks evaluate the scopes attached to the specific assignment that
granted the capability.

```
UserRole:
  user_email: string
  role_name: string

UserRoleScope:
  user_email: string
  role_name: string
  scope_type: "channel" | "registry" | "repository"
  pattern: string              // glob pattern
```

#### Example: three users, one builder role

Roles (capability-only):

| Role | Capabilities |
|------|-------------|
| `admin` | `*` |
| `builder` | `builds:create`, `builds:revoke:own`, `builds:list:own`, `builds:list:any`, `apikeys:create:own` |
| `viewer` | `builds:list:any`, `workers:view` |

Assignments:

| user_email | role_name |
|------------|-----------|
| <joao@clyso.com>m> | admin |
| <alice@clyso.com>m> | builder |
| <bob@clyso.com>m> | builder |
| <bob@clyso.com>m> | viewer |

Per-assignment scopes:

| user_email | role_name | scope_type | pattern |
|------------|-----------|------------|---------|
| <alice@clyso.com>m> | builder | channel | ces-devel/* |
| <alice@clyso.com>m> | builder | registry | harbor.clyso.com/ces-devel/* |
| <bob@clyso.com>m> | builder | channel | * |
| <bob@clyso.com>m> | builder | registry | harbor.clyso.com/* |

Result:

- **joao** — admin, `*` cap, no scope check. Can do everything everywhere.
- **alice** — can build for `ces-devel/*` channels pushing to
  `harbor.clyso.com/ces-devel/*`. A build targeting `ces-prod` → 403.
- **bob** — can build for any channel, any registry under
  `harbor.clyso.com/*`. Also has `viewer` role (no scopes needed — its caps
  are not scope-gated).

#### API shape for assignment with scopes

```json
POST /api/permissions/users/alice@clyso.com/roles
{
  "role": "builder",
  "scopes": [
    { "type": "channel", "pattern": "ces-devel/*" },
    { "type": "registry", "pattern": "harbor.clyso.com/ces-devel/*" }
  ]
}
```

The server rejects this with 400 if the role has scope-dependent capabilities
and `scopes` is empty or missing.

#### Route-level caps removed

The current system has separate "route caps" (`routes:auth:login`,
`routes:builds:status`). These are redundant — if a user has `builds:create`,
they implicitly need access to the build creation endpoint. In the new model,
each endpoint declares which `Cap` it requires, and the extractor checks it
directly. No separate route capability layer.

### Default roles

The system ships with these built-in roles (seeded on first startup):

| Role | Capabilities | Scope-dependent |
|------|-------------|----------------|
| `admin` | `*` | No (global) |
| `builder` | `builds:create`, `builds:revoke:own`, `builds:list:own`, `builds:list:any`, `apikeys:create:own` | Yes — scopes required at assignment |
| `viewer` | `builds:list:any`, `workers:view` | No (global) |

Admins can create additional custom roles.

### First-startup bootstrapping

On first startup (empty database), the server executes a seeding sequence.
This runs exactly once — it has no effect if any data already exists.

**Config:**

```yaml
# cbsd server config
seed_admin: joao.luis@clyso.com
seed_worker_api_keys:
  - name: worker-01
  - name: worker-arm64-01
```

**Seeding order (executed in a single transaction):**

1. Create builtin roles (`admin`, `builder`, `viewer`) with their capabilities.
2. Create user record for `seed_admin` email with `name = "Admin"` (placeholder
   — updated to the real name on first Google OAuth login).
3. Assign the `admin` role to the seed admin user.
4. For each entry in `seed_worker_api_keys`: create an API key owned by the
   seed admin user. Print the plaintext key to stdout. Store the argon2 hash.

**Result:** After first startup, workers can connect immediately using the
printed API keys. The admin user record exists (required by the `api_keys`
foreign key) but has a placeholder name until the admin completes OAuth.

**Alternative for automation:** A CLI mode
`cbsd api-keys create --name worker-01 --db cbsd.db` operates directly on the
database without a running server, enabling scripted provisioning.

### Last admin guard

**Invariant:** At least one **active** user must retain the `*` capability at
all times. This invariant is checked on **every mutation path** that could
violate it:

| Mutation | Guard |
|----------|-------|
| `PUT /permissions/users/{email}/roles` (replace all roles) | Check after replacement |
| `DELETE /permissions/users/{email}/roles/{role}` (remove one role) | Check after removal |
| `PUT /admin/users/{email}/deactivate` (deactivate user) | In transaction: set `active=0`, query remaining active `*` holders, rollback with 409 if count is zero |
| `DELETE /permissions/roles/{name}` (delete role) | Check if role has `*` and CASCADE would remove last holder |
| `PUT /permissions/roles/{name}` (update role capabilities) | See below |

Any operation that would violate the invariant returns **HTTP 409 Conflict**.

**Builtin role protection:** Builtin roles (`builtin = 1`) cannot have their
capabilities modified — `PUT /permissions/roles/{name}` returns 409 for
builtin roles. This prevents stripping `*` from the `admin` role. Custom
roles with `*` are still subject to the last-admin invariant check on
deletion or capability modification.

**Role deletion with assignments:** `DELETE /permissions/roles/{name}` returns
**409 Conflict** if any user-role assignments exist for the role. The admin
must remove assignments first, or pass `?force=true` to trigger CASCADE
deletion (still subject to the last-admin invariant check).

### Ownership enforcement (`:own` vs `:any` capabilities)

Several capabilities come in `:own` / `:any` pairs (e.g., `builds:revoke:own`
and `builds:revoke:any`). The `RequireCap` extractor checks that the caller
has **at least one** of the pair. The handler then enforces ownership:

**Pattern for single-resource endpoints (e.g., `DELETE /builds/{id}`):**

1. The extractor verifies the user has `builds:revoke:own` OR
   `builds:revoke:any`. (Implemented as `AuthUser` + manual
   `user.has_any_cap(&["builds:revoke:own", "builds:revoke:any"])` check,
   since `RequireCap<C>` takes a single cap.)
2. The handler loads the build resource.
3. If the caller has only `:own`, the handler verifies
   `build.user_email == authenticated_user.email`. Mismatch → 403.
4. If the caller has `:any`, ownership check is skipped.

**Pattern for list endpoints (e.g., `GET /builds`):**

- Caller with `builds:list:own`: server filters to
  `WHERE user_email = <caller>`. The `?user=` query parameter is rejected
  with 403.
- Caller with `builds:list:any`: no implicit filter. `?user=` is honored.

This pattern applies to all `:own` / `:any` pairs in the system.

## Database Schema (SQLite)

All state is stored in a single `cbsd.db` SQLite database, accessed via `sqlx`
(async, compile-time checked queries). WAL mode is enabled for concurrent
readers with single writer.

### Auth & permissions tables

All timestamp columns use **INTEGER (Unix epoch seconds)**. Integer timestamps
are more compact, sort correctly without parsing, and avoid timezone
representation bugs.

```sql
-- First statement on connection setup (also in initial migration):
PRAGMA journal_mode=WAL;

-- Users: created on first SSO login
CREATE TABLE users (
    email       TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    active      INTEGER NOT NULL DEFAULT 1,  -- 0 = deactivated by admin
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Tokens: PASETO tokens for human users
-- token_hash uses SHA-256 (not argon2 — see PASETO token hash rationale)
CREATE TABLE tokens (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_email  TEXT NOT NULL REFERENCES users(email),
    token_hash  TEXT NOT NULL UNIQUE,       -- SHA-256 hash of the PASETO token
    expires_at  INTEGER,                    -- NULL = infinite; epoch seconds
    revoked     INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_tokens_user ON tokens(user_email);

-- API keys: for service accounts and workers
-- key_hash uses argon2 (offline brute-force resistance, infrequent validation)
CREATE TABLE api_keys (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,               -- descriptive name
    key_hash    TEXT NOT NULL UNIQUE,         -- argon2 hash
    key_prefix  TEXT NOT NULL,               -- first 12 chars, for user identification
    owner_email TEXT NOT NULL REFERENCES users(email),
    expires_at  INTEGER,                    -- NULL = infinite; epoch seconds
    revoked     INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (owner_email, key_prefix)         -- prevent prefix collisions per owner
);

-- Roles: named permission sets
CREATE TABLE roles (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    builtin     INTEGER NOT NULL DEFAULT 0,  -- 1 = system role, cannot delete
    created_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Role capabilities
-- Capability strings are validated at the API layer against a known enum.
-- Unknown capability strings are rejected with 400 to prevent silent typos.
CREATE TABLE role_caps (
    role_name   TEXT NOT NULL REFERENCES roles(name) ON DELETE CASCADE,
    cap         TEXT NOT NULL,               -- e.g., "builds:create"
    PRIMARY KEY (role_name, cap)
);

-- User-role assignments
-- ON DELETE CASCADE on role_name: deleting a role removes all assignments.
-- This is intentional — the role management API warns before deleting a role
-- with active assignments.
CREATE TABLE user_roles (
    user_email  TEXT NOT NULL REFERENCES users(email) ON DELETE CASCADE,
    role_name   TEXT NOT NULL REFERENCES roles(name) ON DELETE CASCADE,
    PRIMARY KEY (user_email, role_name)
);

-- Per-assignment scopes: scopes live on the assignment, not the role.
-- A role defines what you can do (capabilities). The assignment defines
-- where you can do it (scopes).
CREATE TABLE user_role_scopes (
    user_email  TEXT NOT NULL,
    role_name   TEXT NOT NULL,
    scope_type  TEXT NOT NULL
                CHECK (scope_type IN ('channel', 'registry', 'repository')),
    pattern     TEXT NOT NULL,               -- glob pattern
    FOREIGN KEY (user_email, role_name)
        REFERENCES user_roles(user_email, role_name) ON DELETE CASCADE,
    UNIQUE (user_email, role_name, scope_type, pattern)
);
```

### User deactivation

When a Google account is deactivated or an employee leaves, the admin
deactivates the user via `PUT /api/admin/users/{email}/deactivate` (requires
`permissions:manage`). Reactivation via `PUT /api/admin/users/{email}/activate`.

**Idempotency:** Both activate and deactivate are idempotent. If `active` is
already in the target state, return 200 immediately without running the
last-admin guard or bulk revocation. This prevents the guard from incorrectly
triggering on a no-op (e.g., deactivating an already-deactivated admin).

Deactivation:

1. Sets `users.active = 0`.
2. **Bulk-revokes all tokens** for the user (sets `tokens.revoked = 1`).
3. **Bulk-revokes all API keys** for the user (sets `api_keys.revoked = 1`).
   Also purges any cached API key entries from the LRU cache.

Deactivated users:

- Cannot authenticate (any new token or API key attempt is rejected).
- Retain their user record and build history for auditing.
- Can be reactivated by an admin (`PUT /api/admin/users/{email}/activate`).
  Reactivation restores the user record but does **not** un-revoke tokens or
  API keys — the user must re-authenticate to get new credentials.

There is no self-service account deletion.

### Token revocation details

- **`POST /api/auth/token/revoke`** (no request body): Revokes the token used
  in the current request's `Authorization: Bearer` header. Self-revocation
  only — used for "logout" flows.
- **`POST /api/auth/tokens/revoke-all`** (body: `{"user_email": "..."}`,
  requires `permissions:manage`): Bulk-revokes all tokens for the specified
  user.

**Verb rationale:** Token revocation uses `POST` because it is an action
(invalidating a session), not a resource deletion. API key deletion uses
`DELETE` because it removes a named, listable resource identified by prefix.

### Build tracking tables

```sql
-- Builds: persistent record of every build
CREATE TABLE builds (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    descriptor  TEXT NOT NULL,               -- JSON-serialized BuildDescriptor
    descriptor_version INTEGER NOT NULL DEFAULT 1,  -- schema version (see below)
    user_email  TEXT NOT NULL REFERENCES users(email),
    priority    TEXT NOT NULL DEFAULT 'normal' CHECK (priority IN ('high', 'normal', 'low')),
    state       TEXT NOT NULL DEFAULT 'queued'
                CHECK (state IN ('queued', 'dispatched', 'started',
                                 'revoking', 'success', 'failure', 'revoked')),
    worker_id   TEXT,                        -- which worker ran/is running this
    error       TEXT,                        -- error message if state = 'failure'
    submitted_at INTEGER NOT NULL DEFAULT (unixepoch()),
    queued_at   INTEGER NOT NULL DEFAULT (unixepoch()), -- v1: equals submitted_at;
                                             -- reserved for future deferred queuing
    started_at  INTEGER,                     -- when worker started execution
    finished_at INTEGER                      -- when build completed/failed/revoked
);

CREATE INDEX idx_builds_state ON builds(state);
CREATE INDEX idx_builds_user ON builds(user_email);
CREATE INDEX idx_builds_state_queued ON builds(state, queued_at);  -- startup recovery

-- Build log metadata: tracks the log file for each build.
-- log_path is {log_dir}/builds/{build_id}.log (deterministic from build ID).
-- Stored here for auditability, but functionally derivable.
-- log_path must be updated if log_dir config changes.
CREATE TABLE build_logs (
    build_id    INTEGER PRIMARY KEY REFERENCES builds(id) ON DELETE CASCADE,
    log_path    TEXT NOT NULL,               -- filesystem path to log file
    log_size    INTEGER NOT NULL DEFAULT 0,  -- bytes written so far
    finished    INTEGER NOT NULL DEFAULT 0,  -- 1 = log is complete
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
```

### Descriptor version interpretation

The `builds.descriptor_version` column tracks the JSON schema version of the
`descriptor` blob:

- **Version 1** (default): The `BuildDescriptor` JSON shape as of this design.
  `build.arch` accepts both `arm64` (legacy) and `aarch64` (canonical).
  Python-migrated rows receive `DEFAULT 1`.
- **Unknown versions** (> 1): The server returns an error when attempting to
  deserialize, indicating a server upgrade may be needed. This prevents silent
  misinterpretation of newer descriptor formats by older server versions.

When the `BuildDescriptor` schema evolves, a sqlx migration transforms
existing blobs and bumps the default. Old server versions refuse to read
new-format blobs rather than silently corrupting them.

### Build ID continuity

The Rust server uses SQLite `AUTOINCREMENT` for `builds.id`. On a fresh
database this starts at 1. If migrating from the Python system, the initial
migration must set the autoincrement counter to `MAX(existing_id) + 1` to
avoid collisions with retained Python-era log files.

## REST API — Auth & Permissions

### Authentication endpoints

```
GET  /api/auth/login?client=cli|web     → redirect to Google SSO
GET  /api/auth/callback                 → OAuth callback (creates user + token)
GET  /api/auth/whoami                   → current user info + roles + caps
POST /api/auth/token/revoke             → revoke a token
```

#### `GET /api/auth/whoami` response

```json
{
  "email": "alice@clyso.com",
  "name": "Alice Example",
  "roles": [
    {
      "role": "builder",
      "scopes": [
        { "type": "channel", "pattern": "ces-devel/*" },
        { "type": "registry", "pattern": "harbor.clyso.com/ces-devel/*" }
      ]
    }
  ],
  "effective_caps": ["builds:create", "builds:revoke:own", "builds:list:own",
                      "builds:list:any", "apikeys:create:own"]
}
```

This is the primary source for the user's display name. `cbc` should call
`whoami` after authentication and cache the result locally for constructing
`BuildDescriptor.signed_off_by`.

**Server-side `signed_off_by` override:** The server ignores the client-
submitted `signed_off_by` field in `BuildDescriptor` and overwrites it from
the authenticated user's `users` table record. This prevents identity spoofing
and ensures the build record always matches the authenticated user.

#### PASETO payload schema (frozen)

The encrypted PASETO v4.local payload uses the following schema, versioned as
`CBSD_TOKEN_PAYLOAD_V1`. Both the Python `cbsdcore` and Rust `cbsd` must
produce identical JSON byte sequences for the same logical payload.

**Canonical JSON form (pinned):**

```json
{"expires":1710412200,"user":"alice@clyso.com"}
```

- Keys are **alphabetically ordered** (deterministic serialization).
- `expires`: **Unix epoch seconds** (`i64`), or `null` for infinite TTL.
  Not ISO 8601 — avoids `Z` vs `+00:00` divergence between Pydantic and
  chrono. Epoch integers are unambiguous across all languages.
- `user`: email address (string).
- No `jti` field.
- No whitespace in serialized JSON.

```rust
// Rust — use #[serde(rename_all = "lowercase")] is default; fields are
// already lowercase. Use a custom serializer or serde_json::to_string
// (which outputs alphabetical key order for structs by default).
#[derive(Serialize, Deserialize)]
struct CbsdTokenPayloadV1 {
    expires: Option<i64>,  // epoch seconds, or null
    user: String,          // email — field order matches alphabetical
}
```

```python
# Python — updated cbsdcore token serialization must match:
# json.dumps({"expires": int(dt.timestamp()) if dt else None, "user": email},
#            sort_keys=True, separators=(",", ":"))
```

**Cross-language verification:** A CI test must construct identical payloads in
both Python and Rust and assert SHA-256 equality over the exact byte sequence.
The test must not rely on emergent field ordering — it must verify against
hardcoded expected bytes.

**Divergence with existing Python tokens:** The current Python server
(`cbsd/cbslib/auth/auth.py`) uses `pydantic_core.to_jsonable_python(TokenInfo)`
which serializes `expires` as ISO 8601 (`"2024-03-14T12:30:00+00:00"`), not
epoch integer. The Rust `CBSD_TOKEN_PAYLOAD_V1` uses epoch integers. **These
formats are not hash-compatible.** All existing Python-issued tokens will have
different SHA-256 hashes than the Rust server would compute for the same
logical payload. This is acceptable because the chosen migration strategy is
a hard cutover (users re-authenticate); zero-downtime token import is not
supported for v1.

#### Token hash specification

The `tokens.token_hash` column stores the **SHA-256 hash of the raw UTF-8
PASETO token string** (the full `v4.local.xxx...` string as issued). This is a
frozen specification — both the Python migration script and the Rust server
must hash the same bytes.

#### Bulk token revocation

`POST /api/auth/tokens/revoke-all` (requires `permissions:manage`) revokes all
tokens for a given user email. Used when an account is compromised or
deactivated.

### `GET /builds/{id}` response shape

Field names differ from the current Python server. Listed in the breaking
changes section below.

```json
{
  "id": 42,
  "descriptor": { /* BuildDescriptor JSON */ },
  "descriptor_version": 1,
  "user_email": "alice@clyso.com",
  "priority": "normal",
  "state": "started",
  "worker_id": "worker-arm64-01",
  "error": null,
  "submitted_at": 1710412200,
  "queued_at": 1710412200,
  "started_at": 1710412205,
  "finished_at": null
}
```

**Field changes from Python:** `task_id` → dropped, `submitted` →
`submitted_at` (epoch), `desc` → `descriptor`, `user` → `user_email`,
states are lowercase (`"started"` not `"STARTED"`). These are added to the
coordinated release breaking changes list.

### Error response schema

All error responses use a consistent JSON shape matching the current Python
server (FastAPI's default):

```json
{"detail": "human-readable error message"}
```

HTTP status codes follow the convention documented per-endpoint. If the Rust
server changes this shape, it must be added to the breaking changes for `cbc`.

### API key management

```
POST   /api/auth/api-keys              → create API key (requires apikeys:create:own
                                          or permissions:manage; returns plaintext once)
GET    /api/auth/api-keys              → list own API keys (prefix + metadata)
DELETE /api/auth/api-keys/{prefix}     → revoke API key by prefix (own keys, or any
                                          with permissions:manage)
```

Note: DELETE uses the `key_prefix` (first 12 chars of the random portion,
post-`cbsk_`), not the internal integer ID. Prefix matching is **case-
sensitive** and the canonical form is **lowercase hex**. The `UNIQUE(owner_
email, key_prefix)` constraint prevents collisions per owner. Two different
users may have keys with the same prefix — admin deletion scopes to the
owner's email (either explicit parameter or the calling user).

### Role management (requires permissions:manage)

```
GET    /api/permissions/roles          → list all roles
POST   /api/permissions/roles          → create a custom role
GET    /api/permissions/roles/{name}   → get role details (caps + scopes)
PUT    /api/permissions/roles/{name}   → update role caps/scopes
DELETE /api/permissions/roles/{name}   → delete role (fails for builtin roles)
```

### User-role assignment (requires permissions:manage)

```
GET    /api/permissions/users                    → list users + their roles + scopes
GET    /api/permissions/users/{email}/roles      → list roles for a user + scopes
PUT    /api/permissions/users/{email}/roles      → set roles for a user (replace all)
POST   /api/permissions/users/{email}/roles      → add a role to a user
DELETE /api/permissions/users/{email}/roles/{role}→ remove a role from a user
```

**Scope validation on assignment:** `POST` and `PUT` validate that
scope-dependent roles (those with `builds:create`) include at least one scope.
Requests that would leave scope-dependent capabilities scopeless are rejected
with 400.

**Request body disambiguation for `PUT` (replace-all):**

- `{ "roles": [{"role": "builder", "scopes": [...]}] }` — include builder
  with scopes (accepted if scopes non-empty).
- `{ "roles": [{"role": "builder", "scopes": []}] }` — include builder with
  empty scopes → **rejected** (400, scope-dependent role requires scopes).
- `{ "roles": [{"role": "viewer"}] }` — omit builder entirely → builder
  assignment removed (subject to last-admin guard).

## Migration from Current System

### Permissions migration

The current `permissions.yaml` can be converted to database records at
migration time:

- Each YAML group becomes a role
- Group `authorized_for` entries become role capabilities + scopes
- YAML `rules` entries become user-role assignments
- Regex patterns are converted to equivalent glob patterns

A one-time migration script reads the YAML and populates the SQLite database.
After migration, the YAML file is no longer needed.

### Token migration at cutover

The Rust server starts with an empty `tokens` table. Every PASETO token issued
by the current Python server is unknown to the new server's
`SELECT ... FROM tokens WHERE token_hash = ?` lookup. All existing `cbc` users
will receive 401 on their first API call after cutover.

**Chosen approach:** Accept the cutover break. Notify users in advance that
they must re-authenticate (`cbc login`) after the migration. The new server
uses infinite-TTL tokens by default, so this is a one-time cost.

Note: Zero-downtime token import (migrating Python-era tokens into the Rust
server's `tokens` table) is **not supported for v1**. The Python server
serializes PASETO payloads with ISO 8601 `expires`; the Rust server uses epoch
integers. SHA-256 hashes are incompatible between the two formats.

### API path compatibility

The Rust server uses updated REST API paths (e.g., `POST /api/builds` instead
of `POST /api/builds/new`). The `cbc` client has these paths hardcoded.

**Chosen approach:** Coordinated release. A new `cbc` version with updated
paths is released alongside the Rust server. The minimum compatible `cbc`
version is documented in the server release notes.

Additionally, the Rust server introduces new build states (`dispatched`,
`revoking`, `revoked`) that are absent from the current `cbsdcore.EntryState`
enum. State names are lowercase (current Python uses uppercase). A
new `cbsdcore` release must be published before or alongside the server
release to prevent deserialization errors in `cbc build list`.

**Additional breaking changes for `cbc`:**

- `BuildArch.arm64` → canonical value `aarch64` (Rust server accepts `arm64`
  as alias via `#[serde(alias)]` but serializes as `aarch64`).
- Log streaming: `GET /builds/{id}/logs/follow` returns SSE
  (`text/event-stream`) instead of polled JSON.
- Build state names are lowercase (`queued`, not `QUEUED`).
- New states: `dispatched`, `revoking`, `revoked`.
- `signed_off_by` in build descriptor is overwritten by the server.
- `NewBuildResponse.state` is `"queued"` (lowercase), not `"PENDING"`.
- `GET /builds/{id}` response field changes: `task_id` dropped,
  `submitted` → `submitted_at` (epoch integer), `desc` → `descriptor`,
  `user` → `user_email`.
- Error responses use `{"detail": "..."}` (same shape as Python/FastAPI).

**Release ordering:**

1. Release updated `cbsdcore` (new states, lowercase names)
2. Release updated `cbc` (new paths, SSE log streaming, new states)
3. Deploy Rust server
4. Notify users to update `cbc`
