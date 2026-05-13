# 018 — OpenAPI Integration

## Goal

Expose the cbsd-rs REST API as a browsable, downloadable OpenAPI 3.1
specification. This enables:

1. **Browsable API overview** — developers and operators can explore endpoints,
   request/response shapes, and authentication requirements interactively via a
   `/api/docs` route.
2. **Type stub generation** — the generated OpenAPI JSON serves as input for
   `openapi-typescript` (or similar) to produce TypeScript type stubs for future
   UI integration.

Non-goals for this iteration:

- Client SDK generation pipelines
- API versioning or breaking-change detection
- Request validation driven by the OpenAPI spec

## Approach

### Crate: utoipa

Use **utoipa** with the `axum_extras` feature. utoipa derives OpenAPI metadata
from Rust types and handler signatures at compile time via procedural macros.
Every `cargo build` produces an up-to-date spec without a separate generation
step.

Supporting crates:

| Crate           | Purpose                                           |
| --------------- | ------------------------------------------------- |
| `utoipa`        | `#[derive(ToSchema)]`, `#[utoipa::path]` macros   |
| `utoipa-axum`   | `OpenApiRouter` — axum router that collects paths |
| `utoipa-scalar` | Serves the Scalar API reference UI                |

### Compile-time generation

utoipa's `OpenApi` derive macro assembles the full spec at compile time from all
annotated types and handlers. There is no runtime discovery or reflection — the
spec is baked into the binary. Recompiling the server after any route or type
change automatically regenerates the spec.

### Serving the spec

Two new routes under the existing `/api` namespace:

| Route                    | Method | Response                                   |
| ------------------------ | ------ | ------------------------------------------ |
| `/api/docs`              | GET    | Scalar UI (HTML) — interactive API browser |
| `/api/docs/openapi.json` | GET    | Raw OpenAPI 3.1 JSON document              |

Both routes are **public** (no authentication required) so that developers can
access the spec without a token.

## Annotation Strategy

### Proto types (`cbsd-proto`)

Add `utoipa` as a dependency to `cbsd-proto`. Derive `ToSchema` on all
request/response types that appear in the public API:

- `BuildDescriptor`, `BuildComponent`, `BuildTarget`, `BuildDestImage`,
  `BuildSignedOffBy`
- `BuildState`, `Priority`, `VersionType`, `Arch`
- `BuildId`

Internal-only types (e.g. `WorkerMessage`, `ServerMessage`) are excluded — they
are WebSocket protocol types, not REST API types. `WorkerToken` is also excluded
— it is an internal registration payload, not part of the documented REST
surface.

Types whose fields contain `serde_json::Value` (e.g.
`PeriodicTaskResponse.descriptor`, `CreateRobotBody.expires`) or raw JSON
strings (e.g. `BuildRecord.descriptor`) need explicit `#[schema(...)]`
attributes so the spec documents them as structured objects rather than opaque
strings.

### Server types (`cbsd-server`)

Derive `ToSchema` on route-local request/response structs (e.g. `ErrorDetail`,
`WhoamiResponse`, build list responses, admin responses). Annotate each handler
function with `#[utoipa::path(...)]` specifying:

- HTTP method and path
- Request body / query / path parameter types
- Response status codes and body types
- Security requirements (where applicable)
- Tag (grouping by feature area: auth, builds, workers, admin, periodic,
  channels, components)

### Router integration

Replace `axum::Router` with `utoipa_axum::router::OpenApiRouter` in route
construction. `OpenApiRouter` wraps axum's router and automatically collects
`#[utoipa::path]` metadata from handlers registered via its `.routes()` method.
The top-level router merges all sub-routers and produces the final `OpenApi`
object.

Mount the Scalar UI and JSON endpoint on the merged router. The `OpenApi` object
is wrapped in `Arc` so it can be shared between the Scalar route and the JSON
endpoint without ownership conflicts:

```text
let openapi = Arc::new(openapi);

router
    .merge(Scalar::with_url("/api/docs", openapi.clone()))
    .route("/api/docs/openapi.json", get({
        let spec = openapi.clone();
        move || async move { Json(spec) }
    }))
```

Scalar serves the interactive UI at `/api/docs` and the raw JSON at
`/api/docs/openapi.json`.

## Security Metadata

Define a `SecurityScheme` for the two authentication methods the API supports:

1. **BearerAuth** — `http` scheme, bearer format `PASETO` (API key / token
   authentication)
2. **CookieAuth** — `apiKey` in cookie named `id` (browser session
   authentication)

Handlers that require authentication reference these schemes in their
`#[utoipa::path(security(...))]` annotations. Public endpoints (health, login,
the docs routes themselves) omit the security annotation.

## Implementation Scope

### `cbsd-proto`

- Add `utoipa` dependency (with `chrono` feature for `DateTime` support)
- Derive `ToSchema` on public API types
- No changes to serialization or field names

### `cbsd-server`

- Add `utoipa`, `utoipa-axum`, `utoipa-scalar` dependencies
- Annotate all route handlers with `#[utoipa::path]`
- Derive `ToSchema` on server-local request/response types
- Switch route construction from `axum::Router` to `OpenApiRouter`
- Mount Scalar UI at `/api/docs`
- Define security schemes and API-level metadata (title, version via
  `env!("CARGO_PKG_VERSION")`, description)
- Extract OpenAPI setup into a dedicated `src/openapi.rs` module
- Add a `cargo test` that validates the generated spec without a running server

### What stays unchanged

- `cbsd-worker` and `cbc` — no OpenAPI surface
- Wire format — no serialization changes
- Authentication / middleware — no behavioral changes
- Existing route paths and semantics
