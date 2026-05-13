# 018 — OpenAPI Integration — Design Review v1

Reviews
[018-openapi-integration](../design/018-20260504T1140-openapi-integration.md)
and its
[implementation plan](../plans/018-20260504T1140-openapi-integration.md).

## Verdict

No blockers. Design and plan are ready to implement after incorporating the
findings below. All findings have been accepted and applied to both documents.

## Major Findings

### 1. `submit_build` returns 202, not 201

The plan incorrectly listed `submit_build` as returning 201. The handler returns
`StatusCode::ACCEPTED` (202) because builds are queued, not completed. Fixed in
plan section 3.2.

### 2. `BuildRecord.descriptor` is a raw JSON string

`BuildRecord` and `BuildListRecord` expose `descriptor: String` containing
serialized `BuildDescriptor` JSON. Without annotation the spec would document it
as a plain string. Added `#[schema(value_type = Object)]` guidance to the plan.

## Minor Findings (all accepted)

1. **`WorkerToken` exclusion comment** — add a source comment explaining why it
   is not annotated with `ToSchema`.
2. **WebSocket route exclusion** — `/api/ws/worker` is intentionally excluded;
   added an explicit "Excluded routes" section to the plan.
3. **`logs_full` content type** — plan said "gzipped tar" but handler streams
   raw `application/octet-stream`. Fixed to match implementation.
4. **`queue_status` ad-hoc `Value`** — define a proper `QueueStatusResponse`
   struct with `ToSchema` instead of returning untyped JSON.
5. **`LogsTailQuery.n` schema attributes** — add
   `#[schema(default = 30, maximum = 10000)]` to document bounds.
6. **`robots` tag** — keep `robots` as a standalone tag despite URL nesting
   under `/api/admin/robots`.
7. **`ComponentInfo`** — derive `ToSchema` directly, not inline.

## Suggestions Accepted

1. **`Arc<OpenApi>` ownership** — wrap in `Arc` to share between Scalar and JSON
   routes. Code sketch updated in both design and plan.
2. **`env!("CARGO_PKG_VERSION")`** — read version from crate metadata instead of
   hardcoding. Updated in plan Phase 5.1.
3. **Spec validation test** — add a `#[test]` in `openapi.rs` that serializes
   the spec and asserts valid JSON. Added as plan step 5.3.
4. **`src/openapi.rs` module** — extract OpenAPI setup from `app.rs`. Updated in
   plan Phase 5.1.

## Suggestions Declined

1. **Hoist `utoipa-axum` and `utoipa-scalar` to workspace deps** — only
   `cbsd-server` uses them; keeping them local is correct per workspace
   conventions. `utoipa` itself is shared with `cbsd-proto` and stays in
   workspace deps.
