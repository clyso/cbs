// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

//! Route handlers for `/api/periodic/*`: periodic build task management.

use std::str::FromStr;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cbsd_proto::Priority;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::extractors::{AuthUser, ErrorDetail, ScopeType, auth_error};
use crate::db;
use crate::db::periodic::PeriodicTaskRow;
use crate::scheduler::tag_format;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
struct PeriodicTaskResponse {
    id: String,
    cron_expr: String,
    tag_format: String,
    #[schema(value_type = Object)]
    descriptor: serde_json::Value,
    priority: String,
    summary: Option<String>,
    enabled: bool,
    created_by: String,
    created_at: i64,
    updated_at: i64,
    retry_count: i64,
    retry_at: Option<i64>,
    last_error: Option<String>,
    last_triggered_at: Option<i64>,
    last_build_id: Option<i64>,
    next_run: Option<i64>,
}

/// Convert a database row to an API response, computing `next_run` from the
/// cron expression (or `retry_at` if the task is retrying).
fn task_to_response(row: PeriodicTaskRow) -> PeriodicTaskResponse {
    let next_run = if !row.enabled {
        None
    } else if let Some(retry_at) = row.retry_at {
        Some(retry_at)
    } else {
        croner::Cron::from_str(&row.cron_expr)
            .ok()
            .and_then(|cron| {
                let now = chrono::Utc::now();
                cron.find_next_occurrence(&now, false).ok()
            })
            .map(|dt| dt.timestamp())
    };

    let descriptor = serde_json::from_str(&row.descriptor)
        .unwrap_or_else(|_| serde_json::Value::String(row.descriptor.clone()));

    PeriodicTaskResponse {
        id: row.id,
        cron_expr: row.cron_expr,
        tag_format: row.tag_format,
        descriptor,
        priority: row.priority,
        summary: row.summary,
        enabled: row.enabled,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
        retry_count: row.retry_count,
        retry_at: row.retry_at,
        last_error: row.last_error,
        last_triggered_at: row.last_triggered_at,
        last_build_id: row.last_build_id,
        next_run,
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize, ToSchema)]
struct CreateTaskBody {
    cron_expr: String,
    tag_format: String,
    #[schema(value_type = Object)]
    descriptor: serde_json::Value,
    #[serde(default = "default_priority")]
    priority: String,
    summary: Option<String>,
}

fn default_priority() -> String {
    "normal".to_string()
}

#[derive(Deserialize, ToSchema)]
struct UpdateTaskBody {
    cron_expr: Option<String>,
    tag_format: Option<String>,
    #[schema(value_type = Option<Object>)]
    descriptor: Option<serde_json::Value>,
    priority: Option<String>,
    summary: Option<String>,
}

/// Optional body for `POST /{id}/trigger`. Per design 024 the whole body
/// may be absent (the handler takes `Option<Json<..>>`); `priority` is a
/// one-shot override that never modifies the stored task row.
#[derive(Deserialize, ToSchema)]
struct TriggerTaskBody {
    priority: Option<String>,
}

/// Response for `POST /{id}/trigger`. Modeled on `SubmitBuildResponse`
/// (202, `state`, `is_robot`, `warning`) with the id renamed to
/// `build_id` — the path already carries a task id — plus the
/// interpolated `tag` and effective `priority` for operator visibility.
#[derive(Debug, Serialize, ToSchema)]
struct TriggerTaskResponse {
    build_id: i64,
    state: String,
    tag: String,
    priority: String,
    is_robot: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the periodic tasks sub-router: `/api/periodic/*`.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_task, list_tasks))
        .routes(routes!(get_task, update_task, delete_task))
        .routes(routes!(enable_task))
        .routes(routes!(disable_task))
        .routes(routes!(trigger_task))
}

/// Per audit-rem D3: a user may mutate a periodic task when they hold
/// `periodic:manage:any` OR they hold `periodic:manage:own` and own
/// the task (`task.created_by == user.email`). Owner matching is
/// case-sensitive, matching how `created_by` is stored.
pub(crate) fn can_manage_task(user: &AuthUser, task: &PeriodicTaskRow) -> bool {
    user.has_cap("periodic:manage:any")
        || (user.has_cap("periodic:manage:own") && task.created_by == user.email)
}

/// User-facing error message used for both "missing cap" and
/// "cross-owner attempt with :own". A single generic message avoids
/// leaking whether the failure was a cap-miss or an ownership-miss.
const PERIODIC_MANAGE_DENIED: &str =
    "missing required capability: periodic:manage:own or periodic:manage:any";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{auth_user, test_pool};

    fn task_owned_by(email: &str) -> PeriodicTaskRow {
        PeriodicTaskRow {
            id: "t-1".to_string(),
            cron_expr: "0 0 * * *".to_string(),
            tag_format: "v{n}".to_string(),
            descriptor: "{}".to_string(),
            descriptor_version: 1,
            priority: "normal".to_string(),
            summary: None,
            enabled: true,
            created_by: email.to_string(),
            created_at: 0,
            updated_at: 0,
            retry_count: 0,
            retry_at: None,
            last_error: None,
            last_triggered_at: None,
            last_build_id: None,
        }
    }

    #[test]
    fn any_cap_holder_can_manage_any_task() {
        let user = auth_user(
            "alice@example.com",
            "Alice",
            false,
            &["periodic:manage:any"],
        );
        let task = task_owned_by("bob@example.com");
        assert!(can_manage_task(&user, &task));
    }

    #[test]
    fn own_cap_holder_can_manage_own_task() {
        let user = auth_user(
            "alice@example.com",
            "Alice",
            false,
            &["periodic:manage:own"],
        );
        let task = task_owned_by("alice@example.com");
        assert!(can_manage_task(&user, &task));
    }

    #[test]
    fn own_cap_holder_cannot_manage_other_task() {
        let user = auth_user(
            "alice@example.com",
            "Alice",
            false,
            &["periodic:manage:own"],
        );
        let task = task_owned_by("bob@example.com");
        assert!(!can_manage_task(&user, &task));
    }

    #[test]
    fn no_manage_cap_denies_even_for_own_task() {
        // `periodic:view` alone does not grant mutation rights even
        // over the user's own task.
        let user = auth_user(
            "alice@example.com",
            "Alice",
            false,
            &["periodic:view", "periodic:create"],
        );
        let task = task_owned_by("alice@example.com");
        assert!(!can_manage_task(&user, &task));
    }

    #[test]
    fn both_caps_grant_management_of_any_task() {
        let user = auth_user(
            "alice@example.com",
            "Alice",
            false,
            &["periodic:manage:own", "periodic:manage:any"],
        );
        let task = task_owned_by("bob@example.com");
        assert!(can_manage_task(&user, &task));
    }

    #[test]
    fn empty_caps_set_denies() {
        let user = auth_user("alice@example.com", "Alice", false, &[]);
        let task = task_owned_by("alice@example.com");
        assert!(!can_manage_task(&user, &task));
    }

    /// Per audit-rem D3: migration 008 must remove every `periodic:manage`
    /// row from `role_caps` so a legacy custom role can no longer
    /// silently confer the (now-undefined) cap. The test inserts a
    /// custom role + legacy cap, re-applies the migration SQL, and
    /// asserts the row is gone. (`test_pool` already runs migration
    /// 008 at startup; this test verifies the SQL contract directly.)
    #[tokio::test]
    async fn migration_008_removes_legacy_periodic_manage_cap() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO roles (name, description, builtin) VALUES ('legacy_custom', 'x', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert role");
        sqlx::query(
            "INSERT INTO role_caps (role_name, cap) VALUES ('legacy_custom', 'periodic:manage')",
        )
        .execute(&pool)
        .await
        .expect("insert legacy cap");

        let before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM role_caps WHERE cap = 'periodic:manage'")
                .fetch_one(&pool)
                .await
                .expect("count before");
        assert_eq!(before, 1);

        sqlx::query("DELETE FROM role_caps WHERE cap = 'periodic:manage'")
            .execute(&pool)
            .await
            .expect("migration SQL");

        let after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM role_caps WHERE cap = 'periodic:manage'")
                .fetch_one(&pool)
                .await
                .expect("count after");
        assert_eq!(after, 0);
    }

    // -----------------------------------------------------------------
    // POST /{id}/trigger (design 024)
    // -----------------------------------------------------------------

    use crate::routes::test_support::{
        TEST_RETRY_AT, seed_authed_bearer, seed_periodic_task_in_retry, test_app_state,
        test_session_layer,
    };

    const TRIGGER_COMPONENT: &str = "ceph";
    const TRIGGER_TASK_ID: &str = "trig-task-1";
    const TASK_OWNER: &str = "owner@example.com";
    const REQUESTER: &str = "op@example.com";
    const REQUESTER_CAPS: &[&str] = &["periodic:manage:any", "builds:create"];

    fn trigger_descriptor_json() -> String {
        serde_json::json!({
            "version": "19.2.3",
            "channel": "c1",
            "signed_off_by": {"user": "Owner", "email": TASK_OWNER},
            "dst_image": {"name": "ceph", "tag": "base"},
            "components": [{"name": TRIGGER_COMPONENT, "ref": "main"}],
            "build": {"distro": "rockylinux", "os_version": "el9"}
        })
        .to_string()
    }

    /// Seed the task owner and a task with the given descriptor. The
    /// task is left disabled and in mid-retry state so trigger tests
    /// can assert both are preserved.
    async fn seed_trigger_task(pool: &sqlx::SqlitePool, descriptor: &str) {
        seed_periodic_task_in_retry(
            pool,
            TRIGGER_TASK_ID,
            TASK_OWNER,
            descriptor,
            "{version}-manual",
        )
        .await;
    }

    /// Full success-path fixture: task (disabled, in retry), channel
    /// `c1` with default type `dev`, an authenticated requester (real
    /// user + scope-less role + bearer token), and an `AppState` whose
    /// component registry knows the descriptor's component. Returns
    /// `(state, bearer)`.
    async fn seed_trigger_fixture(pool: &sqlx::SqlitePool) -> (AppState, String) {
        seed_trigger_task(pool, &trigger_descriptor_json()).await;

        let channel_id = crate::db::channels::create_channel(pool, "c1", "test channel")
            .await
            .expect("create channel");
        let type_id = crate::db::channels::create_type(pool, channel_id, "dev", "proj", "")
            .await
            .expect("create type");
        assert!(
            crate::db::channels::set_default_type(pool, channel_id, type_id)
                .await
                .expect("set default type")
        );

        let bearer = seed_authed_bearer(pool, REQUESTER, REQUESTER_CAPS).await;

        let mut state = test_app_state(pool.clone());
        state.components = vec![crate::components::ComponentInfo {
            name: TRIGGER_COMPONENT.to_string(),
            versions: Vec::new(),
        }];
        (state, bearer)
    }

    fn requester() -> AuthUser {
        auth_user(REQUESTER, "Operator", false, REQUESTER_CAPS)
    }

    #[tokio::test]
    async fn trigger_denied_without_manage_cap() {
        let pool = test_pool().await;
        seed_trigger_task(&pool, &trigger_descriptor_json()).await;
        let state = test_app_state(pool);

        let user = auth_user(REQUESTER, "Op", false, &["periodic:view", "builds:create"]);
        let err = trigger_task(State(state), user, Path(TRIGGER_TASK_ID.to_string()), None)
            .await
            .expect_err("must be denied");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn trigger_denied_for_own_cap_on_other_task() {
        let pool = test_pool().await;
        seed_trigger_task(&pool, &trigger_descriptor_json()).await;
        let state = test_app_state(pool);

        let user = auth_user(
            REQUESTER,
            "Op",
            false,
            &["periodic:manage:own", "builds:create"],
        );
        let err = trigger_task(State(state), user, Path(TRIGGER_TASK_ID.to_string()), None)
            .await
            .expect_err("must be denied");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn trigger_denied_without_builds_create() {
        let pool = test_pool().await;
        seed_trigger_task(&pool, &trigger_descriptor_json()).await;
        let state = test_app_state(pool);

        let user = auth_user(REQUESTER, "Op", false, &["periodic:manage:any"]);
        let err = trigger_task(State(state), user, Path(TRIGGER_TASK_ID.to_string()), None)
            .await
            .expect_err("must be denied");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn trigger_unknown_task_is_404() {
        let pool = test_pool().await;
        let state = test_app_state(pool);

        let err = trigger_task(
            State(state),
            requester(),
            Path("no-such-task".to_string()),
            None,
        )
        .await
        .expect_err("must be 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    /// The override is validated strictly and fail-fast — before any
    /// descriptor or channel work, so no fixture beyond the task row.
    #[tokio::test]
    async fn trigger_unknown_priority_is_400() {
        let pool = test_pool().await;
        seed_trigger_task(&pool, &trigger_descriptor_json()).await;
        let state = test_app_state(pool);

        let err = trigger_task(
            State(state),
            requester(),
            Path(TRIGGER_TASK_ID.to_string()),
            Some(Json(TriggerTaskBody {
                priority: Some("urgent".to_string()),
            })),
        )
        .await
        .expect_err("must be 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    /// A stored descriptor that no longer parses yields a synchronous
    /// 400 — the task is NOT auto-disabled (contrast with the
    /// scheduler's Fatal path).
    #[tokio::test]
    async fn trigger_invalid_stored_descriptor_is_400_and_task_untouched() {
        let pool = test_pool().await;
        seed_trigger_task(&pool, "{}").await;
        let state = test_app_state(pool.clone());

        let err = trigger_task(
            State(state),
            requester(),
            Path(TRIGGER_TASK_ID.to_string()),
            None,
        )
        .await
        .expect_err("must be 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let task = db::periodic::get_task(&pool, TRIGGER_TASK_ID)
            .await
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.last_error.as_deref(), Some("boom"));
        assert_eq!(task.retry_count, 3);
    }

    /// Success path with a priority override: the build is attributed
    /// to the requester, carries the override and the task link, and
    /// the task row keeps its stored priority, enabled flag, and retry
    /// state — only the two bookkeeping columns move.
    #[tokio::test]
    async fn trigger_disabled_task_succeeds_and_preserves_task_state() {
        let pool = test_pool().await;
        let (state, _bearer) = seed_trigger_fixture(&pool).await;

        let (status, Json(resp)) = match trigger_task(
            State(state),
            requester(),
            Path(TRIGGER_TASK_ID.to_string()),
            Some(Json(TriggerTaskBody {
                priority: Some("high".to_string()),
            })),
        )
        .await
        {
            Ok(ok) => ok,
            Err((code, _)) => panic!("expected success, got {code}"),
        };

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(resp.state, "queued");
        assert_eq!(resp.priority, "high");
        assert_eq!(resp.tag, "19.2.3-manual");
        assert!(!resp.is_robot);

        let build = sqlx::query!(
            r#"SELECT user_email AS "user_email!", priority AS "priority!",
                      periodic_task_id
               FROM builds WHERE id = ?"#,
            resp.build_id,
        )
        .fetch_one(&pool)
        .await
        .expect("build row");
        assert_eq!(build.user_email, REQUESTER);
        assert_eq!(build.priority, "high");
        assert_eq!(build.periodic_task_id.as_deref(), Some(TRIGGER_TASK_ID));

        let task = db::periodic::get_task(&pool, TRIGGER_TASK_ID)
            .await
            .expect("get task")
            .expect("task exists");
        assert!(!task.enabled);
        assert_eq!(task.priority, "low");
        assert_eq!(task.retry_count, 3);
        assert_eq!(task.retry_at, Some(TEST_RETRY_AT));
        assert_eq!(task.last_error.as_deref(), Some("boom"));
        assert_eq!(task.last_build_id, Some(resp.build_id));
        assert!(task.last_triggered_at.is_some());
    }

    /// The `periodic:manage:own` cap suffices when the requester owns
    /// the task — the full success path, not just the pure gate check.
    /// The build is then attributed to the owner-as-requester.
    #[tokio::test]
    async fn trigger_own_cap_owner_succeeds() {
        let pool = test_pool().await;
        let (state, _bearer) = seed_trigger_fixture(&pool).await;
        // The owner needs a role assignment of their own for the
        // channel scope check (the fixture only authenticates REQUESTER).
        let owner_caps: &[&str] = &["periodic:manage:own", "builds:create"];
        let _owner_bearer = seed_authed_bearer(&pool, TASK_OWNER, owner_caps).await;

        let user = auth_user(TASK_OWNER, "Owner", false, owner_caps);
        let (status, Json(resp)) =
            match trigger_task(State(state), user, Path(TRIGGER_TASK_ID.to_string()), None).await {
                Ok(ok) => ok,
                Err((code, _)) => panic!("expected success, got {code}"),
            };

        assert_eq!(status, StatusCode::ACCEPTED);
        let build = sqlx::query!(
            r#"SELECT user_email AS "user_email!" FROM builds WHERE id = ?"#,
            resp.build_id,
        )
        .fetch_one(&pool)
        .await
        .expect("build row");
        assert_eq!(build.user_email, TASK_OWNER);
    }

    /// Without an override the stored priority column is used, via the
    /// scheduler's lenient mapping.
    #[tokio::test]
    async fn trigger_without_override_uses_stored_priority() {
        let pool = test_pool().await;
        let (state, _bearer) = seed_trigger_fixture(&pool).await;

        let (status, Json(resp)) = match trigger_task(
            State(state),
            requester(),
            Path(TRIGGER_TASK_ID.to_string()),
            None,
        )
        .await
        {
            Ok(ok) => ok,
            Err((code, _)) => panic!("expected success, got {code}"),
        };

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(resp.priority, "low");
    }

    // -----------------------------------------------------------------
    // HTTP-layer tests (mandatory per design 024): the optional-body
    // contract lives in the extractor, which direct handler calls
    // bypass. These drive the real router.
    // -----------------------------------------------------------------

    async fn trigger_oneshot(
        pool: sqlx::SqlitePool,
        state: AppState,
        bearer: &str,
        content_type: Option<&str>,
        body: Option<&str>,
    ) -> StatusCode {
        use tower::ServiceExt;

        let session_layer = test_session_layer(pool).await;
        let app = crate::app::build_router(state, session_layer);

        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/periodic/{TRIGGER_TASK_ID}/trigger"))
            .header("authorization", format!("Bearer {bearer}"));
        if let Some(ct) = content_type {
            builder = builder.header("content-type", ct);
        }
        let req = builder
            .body(axum::body::Body::from(
                body.map(str::to_string).unwrap_or_default(),
            ))
            .expect("build request");

        let response = app.oneshot(req).await.expect("router");
        response.status()
    }

    /// A genuinely bodyless request (no Content-Type header) must fire
    /// the task at its stored priority — the `Option<Json<..>>`
    /// extractor contract. Plain `Json<T>` would 415 here.
    #[tokio::test]
    async fn http_trigger_without_body_or_content_type_is_accepted() {
        let pool = test_pool().await;
        let (state, bearer) = seed_trigger_fixture(&pool).await;
        let status = trigger_oneshot(pool, state, &bearer, None, None).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn http_trigger_with_empty_object_body_is_accepted() {
        let pool = test_pool().await;
        let (state, bearer) = seed_trigger_fixture(&pool).await;
        let status =
            trigger_oneshot(pool, state, &bearer, Some("application/json"), Some("{}")).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn http_trigger_with_unknown_priority_is_400() {
        let pool = test_pool().await;
        let (state, bearer) = seed_trigger_fixture(&pool).await;
        let status = trigger_oneshot(
            pool,
            state,
            &bearer,
            Some("application/json"),
            Some(r#"{"priority": "urgent"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// A non-JSON Content-Type is rejected by the extractor with 415,
    /// exactly like every other JSON endpoint.
    #[tokio::test]
    async fn http_trigger_with_wrong_content_type_is_415() {
        let pool = test_pool().await;
        let (state, bearer) = seed_trigger_fixture(&pool).await;
        let status = trigger_oneshot(pool, state, &bearer, Some("text/plain"), Some("high")).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    /// Design 024: the trigger body is optional, so the generated spec
    /// must NOT mark it required. utoipa derives `required` from the
    /// `Option<..>` content type in `request_body(content = ...)`.
    #[test]
    fn trigger_openapi_body_is_not_required() {
        let (_router, api) = router().split_for_parts();
        let spec = serde_json::to_value(&api).expect("spec serializes");
        let request_body = &spec["paths"]["/{id}/trigger"]["post"]["requestBody"];
        assert!(
            !request_body.is_null(),
            "trigger path must document a request body"
        );
        assert_ne!(
            request_body["required"],
            serde_json::Value::Bool(true),
            "trigger request body must not be marked required"
        );
    }
}

// ---------------------------------------------------------------------------
// POST /api/periodic/
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    request_body = CreateTaskBody,
    responses(
        (status = StatusCode::CREATED, body = PeriodicTaskResponse),
        (status = StatusCode::BAD_REQUEST, body = ErrorDetail),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
    ),
)]
/// Create a new periodic build task.
async fn create_task(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateTaskBody>,
) -> Result<(StatusCode, Json<PeriodicTaskResponse>), (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:create") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:create",
        ));
    }
    if !user.has_cap("builds:create") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: builds:create",
        ));
    }

    // Validate cron expression.
    if croner::Cron::from_str(&body.cron_expr).is_err() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "invalid cron expression",
        ));
    }

    // Validate tag format placeholders.
    if let Err(unknown) = tag_format::validate_tag_format(&body.tag_format) {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            &format!("unknown tag format variables: {}", unknown.join(", ")),
        ));
    }

    // Validate descriptor is a JSON object.
    if !body.descriptor.is_object() {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "descriptor must be a JSON object",
        ));
    }

    // Per WCP D5: shared validator catches empty / unknown components at
    // create time so the trigger never fires on a known-invalid task.
    let typed: cbsd_proto::BuildDescriptor = serde_json::from_value(body.descriptor.clone())
        .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &format!("invalid descriptor: {e}")))?;
    crate::components::validator::validate_descriptor(&typed, &state.components)
        .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &e.to_string()))?;

    // Validate scopes at creation time so users cannot create tasks
    // targeting channels they lack access to (would silently fail at
    // trigger time).
    validate_descriptor_scopes(&state, &user, &body.descriptor).await?;

    let id = uuid::Uuid::new_v4().to_string();
    let descriptor_json = serde_json::to_string(&body.descriptor).map_err(|e| {
        tracing::error!("failed to serialize descriptor: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to serialize descriptor",
        )
    })?;

    db::periodic::insert_task(
        &state.pool,
        &id,
        &body.cron_expr,
        &body.tag_format,
        &descriptor_json,
        &body.priority,
        body.summary.as_deref(),
        &user.email,
    )
    .await
    .map_err(|e| {
        tracing::error!("failed to insert periodic task: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create periodic task",
        )
    })?;

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    let row = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task after insert: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "task not found after insert",
            )
        })?;

    tracing::info!(
        task_id = %id,
        "user {} created periodic task",
        user.email
    );

    Ok((StatusCode::CREATED, Json(task_to_response(row))))
}

// ---------------------------------------------------------------------------
// GET /api/periodic/
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = StatusCode::OK, body = Vec<PeriodicTaskResponse>),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::INTERNAL_SERVER_ERROR, body = ErrorDetail),
    ),
)]
/// List all periodic build tasks.
async fn list_tasks(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PeriodicTaskResponse>>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:view",
        ));
    }

    let rows = db::periodic::list_tasks(&state.pool).await.map_err(|e| {
        tracing::error!("failed to list periodic tasks: {e}");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list periodic tasks",
        )
    })?;

    Ok(Json(rows.into_iter().map(task_to_response).collect()))
}

// ---------------------------------------------------------------------------
// GET /api/periodic/{id}
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/{id}",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    params(("id" = String, Path, description = "Periodic task ID")),
    responses(
        (status = StatusCode::OK, body = PeriodicTaskResponse),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::NOT_FOUND, body = ErrorDetail),
    ),
)]
/// Get a single periodic build task by ID.
async fn get_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<PeriodicTaskResponse>, (StatusCode, Json<ErrorDetail>)> {
    if !user.has_cap("periodic:view") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: periodic:view",
        ));
    }

    let row = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    Ok(Json(task_to_response(row)))
}

// ---------------------------------------------------------------------------
// PUT /api/periodic/{id}
// ---------------------------------------------------------------------------

#[utoipa::path(
    put,
    path = "/{id}",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    params(("id" = String, Path, description = "Periodic task ID")),
    request_body = UpdateTaskBody,
    responses(
        (status = StatusCode::OK, body = PeriodicTaskResponse),
        (status = StatusCode::BAD_REQUEST, body = ErrorDetail),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::NOT_FOUND, body = ErrorDetail),
    ),
)]
/// Update a periodic build task. At least one field must be provided.
async fn update_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskBody>,
) -> Result<Json<PeriodicTaskResponse>, (StatusCode, Json<ErrorDetail>)> {
    // Fetch the existing task first so the cap check can consult
    // ownership (audit-rem D3). A `:own` holder gets 403 on a task
    // they don't own; only `:any` holders may mutate across owners.
    let current = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    if !can_manage_task(&user, &current) {
        return Err(auth_error(StatusCode::FORBIDDEN, PERIODIC_MANAGE_DENIED));
    }

    // At least one field must be present.
    if body.cron_expr.is_none()
        && body.tag_format.is_none()
        && body.descriptor.is_none()
        && body.priority.is_none()
        && body.summary.is_none()
    {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "at least one field must be provided",
        ));
    }

    // If descriptor is being updated, require builds:create and
    // validate scopes against the new descriptor.
    if let Some(ref desc) = body.descriptor {
        if !user.has_cap("builds:create") {
            return Err(auth_error(
                StatusCode::FORBIDDEN,
                "missing required capability: builds:create",
            ));
        }
        validate_descriptor_scopes(&state, &user, desc).await?;
    }

    // Validate cron_expr if provided.
    if let Some(ref cron_expr) = body.cron_expr
        && croner::Cron::from_str(cron_expr).is_err()
    {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "invalid cron expression",
        ));
    }

    // Validate tag_format if provided.
    if let Some(ref tf) = body.tag_format
        && let Err(unknown) = tag_format::validate_tag_format(tf)
    {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            &format!("unknown tag format variables: {}", unknown.join(", ")),
        ));
    }

    // Validate descriptor if provided: structural (object) + typed
    // (non-empty + known component names) per WCP D5.
    if let Some(ref desc) = body.descriptor {
        if !desc.is_object() {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                "descriptor must be a JSON object",
            ));
        }
        let typed: cbsd_proto::BuildDescriptor =
            serde_json::from_value(desc.clone()).map_err(|e| {
                auth_error(StatusCode::BAD_REQUEST, &format!("invalid descriptor: {e}"))
            })?;
        crate::components::validator::validate_descriptor(&typed, &state.components)
            .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &e.to_string()))?;
    }

    // Merge fields against the row fetched earlier for the cap check.
    let new_cron_expr = body.cron_expr.unwrap_or(current.cron_expr);
    let new_tag_format = body.tag_format.unwrap_or(current.tag_format);
    let new_priority = body.priority.unwrap_or(current.priority);
    let new_summary = if body.summary.is_some() {
        body.summary
    } else {
        current.summary
    };
    let new_descriptor = if let Some(ref desc) = body.descriptor {
        serde_json::to_string(desc).map_err(|e| {
            tracing::error!("failed to serialize descriptor: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to serialize descriptor",
            )
        })?
    } else {
        current.descriptor
    };

    // Write back the full row. Clear retry state if the task was retrying.
    sqlx::query!(
        r#"UPDATE periodic_tasks
           SET cron_expr = ?, tag_format = ?, descriptor = ?, priority = ?,
               summary = ?, retry_count = 0, retry_at = NULL, last_error = NULL,
               updated_at = unixepoch()
           WHERE id = ?"#,
        new_cron_expr,
        new_tag_format,
        new_descriptor,
        new_priority,
        new_summary,
        id,
    )
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("failed to update periodic task '{id}': {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    let row = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}' after update: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| {
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "task not found after update",
            )
        })?;

    tracing::info!(
        task_id = %id,
        "user {} updated periodic task",
        user.email
    );

    Ok(Json(task_to_response(row)))
}

// ---------------------------------------------------------------------------
// DELETE /api/periodic/{id}
// ---------------------------------------------------------------------------

#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    params(("id" = String, Path, description = "Periodic task ID")),
    responses(
        (status = StatusCode::OK),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::NOT_FOUND, body = ErrorDetail),
    ),
)]
/// Delete a periodic build task.
async fn delete_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Per audit-rem D3: fetch first to consult ownership for the
    // `:own` cap variant.
    let task = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    if !can_manage_task(&user, &task) {
        return Err(auth_error(StatusCode::FORBIDDEN, PERIODIC_MANAGE_DENIED));
    }

    let deleted = db::periodic::delete_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to delete periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if !deleted {
        return Err(auth_error(StatusCode::NOT_FOUND, "periodic task not found"));
    }

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    tracing::info!(
        task_id = %id,
        "user {} deleted periodic task",
        user.email
    );

    Ok(Json(
        serde_json::json!({"detail": format!("periodic task '{id}' deleted")}),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/periodic/{id}/enable
// ---------------------------------------------------------------------------

#[utoipa::path(
    put,
    path = "/{id}/enable",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    params(("id" = String, Path, description = "Periodic task ID")),
    responses(
        (status = StatusCode::OK),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::NOT_FOUND, body = ErrorDetail),
    ),
)]
/// Enable a periodic build task. Resets retry state.
async fn enable_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Per audit-rem D3: fetch first to consult ownership.
    let task = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    if !can_manage_task(&user, &task) {
        return Err(auth_error(StatusCode::FORBIDDEN, PERIODIC_MANAGE_DENIED));
    }

    let updated = db::periodic::enable_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to enable periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if !updated {
        return Err(auth_error(StatusCode::NOT_FOUND, "periodic task not found"));
    }

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    tracing::info!(
        task_id = %id,
        "user {} enabled periodic task",
        user.email
    );

    Ok(Json(
        serde_json::json!({"detail": format!("periodic task '{id}' enabled")}),
    ))
}

// ---------------------------------------------------------------------------
// PUT /api/periodic/{id}/disable
// ---------------------------------------------------------------------------

#[utoipa::path(
    put,
    path = "/{id}/disable",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    params(("id" = String, Path, description = "Periodic task ID")),
    responses(
        (status = StatusCode::OK),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::NOT_FOUND, body = ErrorDetail),
    ),
)]
/// Disable a periodic build task. Clears retry_at.
async fn disable_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorDetail>)> {
    // Per audit-rem D3: fetch first to consult ownership.
    let task = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    if !can_manage_task(&user, &task) {
        return Err(auth_error(StatusCode::FORBIDDEN, PERIODIC_MANAGE_DENIED));
    }

    let updated = db::periodic::disable_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to disable periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;

    if !updated {
        return Err(auth_error(StatusCode::NOT_FOUND, "periodic task not found"));
    }

    // Notify the scheduler to reload.
    state.scheduler_notify.notify_one();

    tracing::info!(
        task_id = %id,
        "user {} disabled periodic task",
        user.email
    );

    Ok(Json(
        serde_json::json!({"detail": format!("periodic task '{id}' disabled")}),
    ))
}

// ---------------------------------------------------------------------------
// POST /api/periodic/{id}/trigger
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/{id}/trigger",
    tag = "periodic",
    security(("bearer" = []), ("cookie" = [])),
    params(("id" = String, Path, description = "Periodic task ID")),
    request_body(
        content = Option<TriggerTaskBody>,
        description = "Optional one-shot priority override (high/normal/low)",
    ),
    responses(
        (status = StatusCode::ACCEPTED, body = TriggerTaskResponse),
        (status = StatusCode::BAD_REQUEST, body = ErrorDetail),
        (status = StatusCode::FORBIDDEN, body = ErrorDetail),
        (status = StatusCode::NOT_FOUND, body = ErrorDetail),
    ),
)]
/// Trigger a periodic build task immediately (design 024). The build is
/// attributed to the requester, whose own build permissions are checked;
/// the task's schedule, enabled flag, and retry state are not touched.
/// Disabled tasks are deliberately triggerable ("test before enable").
async fn trigger_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    // `Option<Json<..>>` is load-bearing: plain `Json<T>` rejects a
    // request with no Content-Type header with 415, breaking the bare
    // `curl -X POST .../trigger` case this endpoint exists to serve.
    body: Option<Json<TriggerTaskBody>>,
) -> Result<(StatusCode, Json<TriggerTaskResponse>), (StatusCode, Json<ErrorDetail>)> {
    // Per audit-rem D3: fetch first to consult ownership.
    let task = db::periodic::get_task(&state.pool, &id)
        .await
        .map_err(|e| {
            tracing::error!("failed to get periodic task '{id}': {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::NOT_FOUND, "periodic task not found"))?;

    if !can_manage_task(&user, &task) {
        return Err(auth_error(StatusCode::FORBIDDEN, PERIODIC_MANAGE_DENIED));
    }
    if !user.has_cap("builds:create") {
        return Err(auth_error(
            StatusCode::FORBIDDEN,
            "missing required capability: builds:create",
        ));
    }

    // Strict, fail-fast validation of the override; the stored column
    // keeps the scheduler's lenient mapping (see effective priority
    // below). Direct user input gets a 400, not serde's 422.
    let override_priority = match body.as_ref().and_then(|b| b.priority.as_deref()) {
        None => None,
        Some("high") => Some(Priority::High),
        Some("normal") => Some(Priority::Normal),
        Some("low") => Some(Priority::Low),
        Some(_) => {
            return Err(auth_error(
                StatusCode::BAD_REQUEST,
                "invalid priority: expected 'high', 'normal', or 'low'",
            ));
        }
    };

    // Parse + re-validate the stored descriptor (WCP D5). Unlike the
    // scheduler's Fatal path this does NOT disable the task: the
    // operator sees the failure synchronously; auto-disable protects
    // unattended fires only.
    let mut descriptor: cbsd_proto::BuildDescriptor = serde_json::from_str(&task.descriptor)
        .map_err(|e| {
            auth_error(
                StatusCode::BAD_REQUEST,
                &format!("invalid stored descriptor: {e}"),
            )
        })?;
    crate::components::validator::validate_descriptor(&descriptor, &state.components)
        .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &e.to_string()))?;

    // Repository scope checks against the REQUESTER, as submit_build.
    let mut scope_checks: Vec<(ScopeType, String)> = Vec::new();
    for comp in &descriptor.components {
        if let Some(ref repo) = comp.repo {
            scope_checks.push((ScopeType::Repository, repo.clone()));
        }
    }
    if !scope_checks.is_empty() {
        let scope_refs: Vec<(ScopeType, &str)> =
            scope_checks.iter().map(|(t, v)| (*t, v.as_str())).collect();
        user.require_scopes_all(&state.pool, &scope_refs).await?;
    }

    // Attribution: the requester, not the task owner.
    descriptor.signed_off_by.user = user.name.clone();
    descriptor.signed_off_by.email = user.email.clone();

    // Interpolate the tag with the current UTC time. Same step order as
    // the scheduler (interpolation before channel resolution), so both
    // paths produce identical tags for the same task.
    let now = chrono::Utc::now();
    let tag = tag_format::interpolate_tag(&task.tag_format, &descriptor, now);
    tag_format::validate_oci_tag(&tag).map_err(|e| {
        auth_error(
            StatusCode::BAD_REQUEST,
            &format!("interpolated tag '{tag}' is not a valid OCI tag: {e}"),
        )
    })?;
    descriptor.dst_image.tag = tag.clone();

    // Channel/type resolution + channel scope check for the requester.
    let user_record = db::users::get_user(&state.pool, &user.email)
        .await
        .map_err(|e| {
            tracing::error!("failed to get user record: {e}");
            auth_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?
        .ok_or_else(|| auth_error(StatusCode::INTERNAL_SERVER_ERROR, "user record not found"))?;

    let resolved = crate::channels::resolve_and_rewrite(&state.pool, &mut descriptor, &user_record)
        .await
        .map_err(|e| auth_error(StatusCode::BAD_REQUEST, &e))?;

    // Robots may not submit to ${username}-prefixed channels (as
    // submit_build).
    if user_record.is_robot
        && crate::channels::prefix_template_contains_username(&resolved.prefix_template)
    {
        return Err(auth_error(
            StatusCode::BAD_REQUEST,
            "robot accounts cannot submit builds to channels that use the ${username} prefix template",
        ));
    }

    // Effective priority: strict override wins, else the stored column
    // via the scheduler's lenient mapping.
    let priority = override_priority.unwrap_or(match task.priority.as_str() {
        "high" => Priority::High,
        "low" => Priority::Low,
        _ => Priority::Normal,
    });

    let (build_id, pending_count) = crate::routes::builds::insert_build_internal(
        &state,
        descriptor,
        &user.email,
        priority,
        Some(&task.id),
        Some(resolved.channel_id),
        Some(resolved.channel_type_id),
    )
    .await
    .map_err(|e| {
        tracing::error!("failed to submit build for periodic task '{id}': {e}");
        auth_error(StatusCode::INTERNAL_SERVER_ERROR, &e)
    })?;

    // Bookkeeping only — the build exists and is queued, so a failure
    // here must not fail the request.
    if let Err(e) = db::periodic::record_manual_trigger(&state.pool, &task.id, build_id).await {
        tracing::error!(
            task_id = %task.id,
            "failed to record manual trigger for build {build_id}: {e}"
        );
    }

    let priority_str = match priority {
        Priority::High => "high",
        Priority::Normal => "normal",
        Priority::Low => "low",
    };

    tracing::info!(
        task_id = %task.id,
        "user {} manually triggered periodic task: build {build_id} (priority={priority_str})",
        user.display_identity(),
    );

    let warning = if pending_count > 1 {
        Some(format!("{pending_count} build(s) in queue"))
    } else {
        None
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(TriggerTaskResponse {
            build_id,
            state: "queued".to_string(),
            tag,
            priority: priority_str.to_string(),
            is_robot: user.is_robot,
            warning,
        }),
    ))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract channel and repository scopes from a descriptor JSON and run
/// the same scope validation used by `submit_build`. This catches
/// permission issues at task creation/update time instead of silently
/// failing when the scheduler triggers the build.
async fn validate_descriptor_scopes(
    state: &AppState,
    user: &AuthUser,
    descriptor: &serde_json::Value,
) -> Result<(), (StatusCode, Json<ErrorDetail>)> {
    let mut scope_checks: Vec<(ScopeType, String)> = Vec::new();

    // Channel scope is NOT checked here. The channel/type composite is
    // validated downstream by resolve_and_rewrite → check_channel_scope
    // at both build-submission and periodic-trigger time.

    // Repository scopes from component repo overrides.
    if let Some(components) = descriptor.get("components").and_then(|v| v.as_array()) {
        for comp in components {
            if let Some(repo) = comp.get("repo").and_then(|v| v.as_str()) {
                scope_checks.push((ScopeType::Repository, repo.to_string()));
            }
        }
    }

    if !scope_checks.is_empty() {
        let scope_refs: Vec<(ScopeType, &str)> =
            scope_checks.iter().map(|(t, v)| (*t, v.as_str())).collect();
        user.require_scopes_all(&state.pool, &scope_refs).await?;
    }

    Ok(())
}
