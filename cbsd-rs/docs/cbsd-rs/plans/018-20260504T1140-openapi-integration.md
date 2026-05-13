# 018 — OpenAPI Integration Plan

Implements
[018-openapi-integration](../design/018-20260504T1140-openapi-integration.md).

## Phase 1 — Dependencies & Proto Annotations

**Goal:** Add utoipa to the workspace and annotate all REST-facing proto types
with `ToSchema`.

### 1.1 Add workspace dependency

In `cbsd-rs/Cargo.toml` (workspace root), add under `[workspace.dependencies]`:

```toml
utoipa = { version = "5", features = ["chrono"] }
```

### 1.2 Wire utoipa into `cbsd-proto`

In `cbsd-proto/Cargo.toml`, add:

```toml
utoipa = { workspace = true }
```

### 1.3 Derive `ToSchema` on REST-facing proto types

Add `#[derive(utoipa::ToSchema)]` to these types in `cbsd-proto/src/`:

| File       | Types                                                                                                                                        |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `arch.rs`  | `Arch`                                                                                                                                       |
| `build.rs` | `BuildId`, `Priority`, `BuildState`, `VersionType`, `BuildSignedOffBy`, `BuildDestImage`, `BuildComponent`, `BuildTarget`, `BuildDescriptor` |

Skip WebSocket-only types: `ServerMessage`, `WorkerMessage`,
`WorkerReportedState`, `BuildFinishedStatus`.

Skip `WorkerToken` — it is an internal registration payload, not part of the
documented REST surface. Add a source comment on `WorkerToken` explaining the
exclusion to prevent future contributors from adding it.

### 1.4 Verify

```bash
cd cbsd-rs && cargo check --workspace
```

---

## Phase 2 — Server Dependencies & Schema Annotations

**Goal:** Add utoipa crates to cbsd-server and derive `ToSchema` on all
server-local request/response types.

### 2.1 Add server dependencies

In `cbsd-server/Cargo.toml`:

```toml
utoipa = { workspace = true }
utoipa-axum = "0.2"
utoipa-scalar = { version = "0.3", features = ["axum"] }
```

### 2.2 Derive `ToSchema` on server types

Add `#[derive(utoipa::ToSchema)]` to every request/response struct used in route
handlers:

| File                    | Types                                                                                                                                                                                |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `auth/extractors.rs`    | `ErrorDetail`                                                                                                                                                                        |
| `routes/auth.rs`        | `WhoamiResponse`, `RevokeAllBody`, `CreateApiKeyBody`, `CreateApiKeyResponse`, `ApiKeyItem`                                                                                          |
| `routes/builds.rs`      | `SubmitBuildBody`, `SubmitBuildResponse`, `ListBuildsQuery`, `LogsTailQuery`                                                                                                         |
| `routes/admin.rs`       | `RegisterWorkerBody`, `RegisterWorkerResponse`, `SetDefaultChannelBody`, `EntityRoleItem`, `EntityWithRolesItem`, `ReplaceEntityRolesBody`, `AddEntityRoleBody`, `ListEntitiesQuery` |
| `routes/robots.rs`      | `CreateRobotBody`, `CreateRobotResponse`, `RotateTokenBody`, `RotateTokenResponse`, `SetDescriptionBody`, `RobotListItem`, `RobotDetail`, `TokenStatusBody`                          |
| `routes/permissions.rs` | `CreateRoleBody`, `RoleResponse`, `RoleListItem`, `ScopeBody`                                                                                                                        |
| `routes/workers.rs`     | `WorkerInfoResponse`                                                                                                                                                                 |
| `routes/channels.rs`    | `CreateChannelBody`, `UpdateChannelBody`, `AddTypeBody`, `UpdateTypeBody`, `SetDefaultTypeBody`, `ChannelResponse`, `TypeResponse`                                                   |
| `routes/periodic.rs`    | `PeriodicTaskResponse`, `CreateTaskBody`, `UpdateTaskBody`                                                                                                                           |

Types that use `serde_json::Value` (e.g. `PeriodicTaskResponse.descriptor`,
`CreateRobotBody.expires`) need a `#[schema(value_type = Object)]` attribute to
map to an opaque JSON object in the spec.

The `BuildListRecord` and `BuildRecord` types returned by list/get build
handlers also need `ToSchema` — locate their definitions (likely constructed
inline from DB rows) and either derive or manually define schemas. Note:
`BuildRecord.descriptor` is a raw JSON string at the DB layer containing
serialized `BuildDescriptor`. Annotate with
`#[schema(value_type = Object, description = "Serialized BuildDescriptor; see BuildDescriptor schema")]`
so the spec documents it as structured data.

`LogsTailQuery.n` has a serde default invisible to utoipa. Add
`#[schema(default = 30, maximum = 10000)]` (or the actual values) to document
bounds in the spec.

### 2.3 Verify

```bash
cd cbsd-rs && cargo check -p cbsd-server
```

---

## Phase 3 — Handler Annotations (by route module)

**Goal:** Annotate every handler with `#[utoipa::path]`. Work through one route
file at a time. Each handler gets method, path, tag, request body / query / path
params, response codes, and security.

Security references (defined later in Phase 5):

- Authenticated endpoints: `security(("bearer" = []), ("cookie" = []))`
- Public endpoints (health, login, callback, docs): omit security

### 3.1 `routes/auth.rs`

| Handler                  | Tag  | Notes                              |
| ------------------------ | ---- | ---------------------------------- |
| `login`                  | auth | Public; GET with `LoginQuery`      |
| `callback`               | auth | Public; GET with `CallbackQuery`   |
| `logout`                 | auth | POST; authenticated                |
| `whoami`                 | auth | GET; returns `WhoamiResponse`      |
| `revoke_token`           | auth | POST; 204 on success               |
| `revoke_all_tokens`      | auth | POST; body `RevokeAllBody`         |
| `create_api_key_handler` | auth | POST; 201 + `CreateApiKeyResponse` |
| `list_api_keys_handler`  | auth | GET; returns `Vec<ApiKeyItem>`     |
| `revoke_api_key_handler` | auth | DELETE with path `{prefix}`        |

### 3.2 `routes/builds.rs`

| Handler        | Tag    | Notes                                                      |
| -------------- | ------ | ---------------------------------------------------------- |
| `submit_build` | builds | POST; 202 + `SubmitBuildResponse`                          |
| `list_builds`  | builds | GET with `ListBuildsQuery`                                 |
| `get_build`    | builds | GET `{id}`; returns `BuildRecord`                          |
| `revoke_build` | builds | DELETE `{id}`                                              |
| `logs_tail`    | builds | GET `{id}/logs/tail` with `LogsTailQuery`                  |
| `logs_follow`  | builds | GET `{id}/logs/follow`; SSE stream (`text/event-stream`)   |
| `logs_full`    | builds | GET `{id}/logs`; raw log file (`application/octet-stream`) |

For streaming responses (`logs_follow`, `logs_full`), use
`#[utoipa::path(responses(...))]` with content-type overrides rather than a
typed body. Document `logs_follow` as `text/event-stream` and `logs_full` as
`application/octet-stream`.

### 3.3 `routes/admin.rs`

| Handler                      | Tag   | Notes                                                                              |
| ---------------------------- | ----- | ---------------------------------------------------------------------------------- |
| `deactivate_entity`          | admin | PUT `entity/{email}/deactivate`                                                    |
| `activate_entity`            | admin | PUT `entity/{email}/activate`                                                      |
| `set_entity_default_channel` | admin | PUT; body `SetDefaultChannelBody`                                                  |
| `get_entity_roles`           | admin | GET; returns `Vec<EntityRoleItem>`                                                 |
| `replace_entity_roles`       | admin | PUT; body `ReplaceEntityRolesBody`                                                 |
| `add_entity_role`            | admin | POST; 201 + `EntityRoleItem`                                                       |
| `remove_entity_role`         | admin | DELETE `entity/{email}/roles/{role}`                                               |
| `list_entities`              | admin | GET with `ListEntitiesQuery`                                                       |
| `queue_status`               | admin | GET; define `QueueStatusResponse` struct with `ToSchema` to replace ad-hoc `Value` |
| `register_worker`            | admin | POST; 201 + `RegisterWorkerResponse`                                               |
| `deregister_worker`          | admin | DELETE `workers/{id}`                                                              |
| `regenerate_worker_token`    | admin | POST; returns `RegisterWorkerResponse`                                             |

### 3.4 `routes/robots.rs`

| Handler                  | Tag    | Notes                               |
| ------------------------ | ------ | ----------------------------------- |
| `create_or_revive_robot` | robots | POST; 201 + `CreateRobotResponse`   |
| `list_robots`            | robots | GET; returns `Vec<RobotListItem>`   |
| `get_robot`              | robots | GET `{name}`; returns `RobotDetail` |
| `tombstone_robot`        | robots | DELETE `{name}`                     |
| `create_or_rotate_token` | robots | POST; body `RotateTokenBody`        |
| `revoke_robot_token`     | robots | DELETE `{name}/token`               |
| `set_robot_description`  | robots | PUT; body `SetDescriptionBody`      |

### 3.5 `routes/permissions.rs`

| Handler       | Tag         | Notes                                |
| ------------- | ----------- | ------------------------------------ |
| `list_roles`  | permissions | GET; returns `Vec<RoleListItem>`     |
| `create_role` | permissions | POST; 201 + `RoleResponse`           |
| `get_role`    | permissions | GET `{name}`; returns `RoleResponse` |
| `update_role` | permissions | PUT `{name}`; body `CreateRoleBody`  |
| `delete_role` | permissions | DELETE `{name}`                      |

### 3.6 `routes/workers.rs`

| Handler        | Tag     | Notes                                  |
| -------------- | ------- | -------------------------------------- |
| `list_workers` | workers | GET; returns `Vec<WorkerInfoResponse>` |

### 3.7 `routes/components.rs`

| Handler           | Tag        | Notes                             |
| ----------------- | ---------- | --------------------------------- |
| `list_components` | components | GET; returns `Vec<ComponentInfo>` |

`ComponentInfo` is loaded from YAML at startup — derive `ToSchema` on it
directly (do not use an inline schema, as it will drift).

### 3.8 `routes/channels.rs`

| Handler            | Tag      | Notes                                   |
| ------------------ | -------- | --------------------------------------- |
| `create_channel`   | channels | POST; 201 + `ChannelResponse`           |
| `list_channels`    | channels | GET; returns `Vec<ChannelResponse>`     |
| `get_channel`      | channels | GET `{id}`                              |
| `update_channel`   | channels | PUT `{id}`; body `UpdateChannelBody`    |
| `delete_channel`   | channels | DELETE `{id}`                           |
| `add_type`         | channels | POST `{id}/types`; 201 + `TypeResponse` |
| `update_type`      | channels | PUT `{id}/types/{tid}`                  |
| `delete_type`      | channels | DELETE `{id}/types/{tid}`               |
| `set_default_type` | channels | PUT `{id}/default-type`                 |

### 3.9 `routes/periodic.rs`

| Handler        | Tag      | Notes                              |
| -------------- | -------- | ---------------------------------- |
| `create_task`  | periodic | POST; 201 + `PeriodicTaskResponse` |
| `list_tasks`   | periodic | GET                                |
| `get_task`     | periodic | GET `{id}`                         |
| `update_task`  | periodic | PUT `{id}`                         |
| `delete_task`  | periodic | DELETE `{id}`                      |
| `enable_task`  | periodic | PUT `{id}/enable`                  |
| `disable_task` | periodic | PUT `{id}/disable`                 |

### 3.10 `app.rs` — health endpoint

| Handler      | Tag    | Notes                     |
| ------------ | ------ | ------------------------- |
| health check | system | GET `/api/health`; public |

### Excluded routes

The WebSocket upgrade route `/api/ws/worker` is intentionally excluded from the
OpenAPI spec. It is a worker-internal protocol endpoint, not a REST API surface.

---

## Phase 4 — Router Migration

**Goal:** Replace `axum::Router` with `utoipa_axum::router::OpenApiRouter` so
that `#[utoipa::path]` metadata is automatically collected.

### 4.1 Update route module `router()` functions

Each `routes/*.rs` file has a `pub fn router() -> Router<AppState>`. Change the
return type to `OpenApiRouter<AppState>` and replace `.route()` calls with
`.routes()` (utoipa-axum's method that accepts handler + path metadata).

Work through each module in the same order as Phase 3.

### 4.2 Update `app.rs`

- Import `utoipa_axum::router::OpenApiRouter`
- Build the top-level router as `OpenApiRouter`
- Nest sub-routers using `.nest()`
- After all routes are assembled, split into `(Router, OpenApi)` using
  `.split_for_parts()`
- The `Router` is used for serving; the `OpenApi` feeds Phase 5

### 4.3 Verify

```bash
cd cbsd-rs && cargo check -p cbsd-server
```

Confirm that the existing test suite still passes:

```bash
cd cbsd-rs && cargo test --workspace
```

---

## Phase 5 — OpenAPI Metadata & Docs Routes

**Goal:** Define the `OpenApi` top-level metadata, security schemes, and mount
the Scalar UI + JSON endpoint.

### 5.1 Define `OpenApi` struct

Create a new `src/openapi.rs` module. Use `#[derive(OpenApi)]` to declare
API-level metadata. Read the version from `env!("CARGO_PKG_VERSION")` to keep it
in sync with the crate version automatically:

```rust
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "CBS Build Service",
        description = "REST API for the CES Build System daemon"
    ),
    security(
        ("bearer" = []),
        ("cookie" = [])
    ),
    modifiers(&SecurityAddon)
)]
struct ApiDoc;
```

Set the version programmatically via a modifier or by mutating the `OpenApi`
object after construction using `env!("CARGO_PKG_VERSION")`.

Implement a `SecurityAddon` modifier that registers:

- `bearer` — `SecurityScheme::Http` with scheme `bearer`, bearer format `PASETO`
- `cookie` — `SecurityScheme::ApiKey` with `ApiKeyValue` in cookie named `id`

### 5.2 Mount Scalar UI

After splitting the router, wrap the `OpenApi` in `Arc` to share it between the
Scalar route and the JSON endpoint:

```rust
let (router, openapi) = api_router.split_for_parts();
let openapi = Arc::new(openapi);

let router = router
    .merge(Scalar::with_url("/api/docs", openapi.clone()))
    .route("/api/docs/openapi.json", get({
        let spec = openapi.clone();
        move || async move { Json(spec) }
    }));
```

Both routes are public — no auth middleware applied.

### 5.3 Add spec validation test

Add a `#[test]` in `openapi.rs` that calls `ApiDoc::openapi()`, serializes to
JSON, and asserts it parses as valid JSON. This catches spec regressions in
`cargo test` without a running server.

### 5.4 Verify end-to-end

1. `cargo build -p cbsd-server`
2. `cargo test -p cbsd-server` — confirm spec validation test passes
3. Start the server locally
4. `curl http://localhost:PORT/api/docs/openapi.json | jq .` — confirm valid
   OpenAPI 3.1 JSON with all paths, schemas, and security
5. Open `http://localhost:PORT/api/docs` in a browser — confirm Scalar UI
   renders with all endpoint groups

---

## Phase 6 — Cleanup & CI

### 6.1 Format and lint

```bash
cargo fmt --all
cargo clippy --workspace
```

### 6.2 Offline query cache

If sqlx offline mode is used in CI:

```bash
DATABASE_URL=sqlite:///tmp/cbsd-dev.db cargo sqlx prepare --workspace
```

### 6.3 Verify full build

```bash
SQLX_OFFLINE=true cargo check --workspace
cargo test --workspace
```

---

## Commit Strategy

| Commit | Scope                                                                                                                 |
| ------ | --------------------------------------------------------------------------------------------------------------------- |
| 1      | Phase 1 — utoipa dep + proto `ToSchema` derives                                                                       |
| 2      | Phase 2 — server deps + server type `ToSchema` derives                                                                |
| 3      | Phase 3 + 4 — handler annotations + router migration (one commit per route module is acceptable if the diff is large) |
| 4      | Phase 5 — OpenApi metadata, security schemes, Scalar mount                                                            |
| 5      | Phase 6 — fmt, clippy, CI fixups                                                                                      |
