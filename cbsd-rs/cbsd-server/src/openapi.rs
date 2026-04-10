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

//! OpenAPI spec assembly and Swagger UI serving.
//!
//! Two routes are registered (outside the `/api` prefix, no auth required):
//! - `GET /api/docs`             — Swagger UI HTML (loads spec via CDN)
//! - `GET /api/docs/openapi.json` — raw OpenAPI 3.x JSON spec

use axum::Router;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use utoipa::OpenApi;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};

use crate::auth::extractors::ErrorDetail;
use crate::components::ComponentInfo;
use crate::db::builds::{BuildListRecord, BuildRecord};

/// Base OpenAPI document: info, security schemes, and shared schemas.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "CBS API",
        version = "0.1.0",
        description = "CES Build System — REST API for the cbsd-rs build service daemon",
        license(name = "AGPL-3.0-or-later"),
    ),
    components(
        schemas(
            // cbsd-proto
            cbsd_proto::Arch,
            cbsd_proto::Priority,
            cbsd_proto::BuildState,
            cbsd_proto::VersionType,
            cbsd_proto::BuildId,
            cbsd_proto::BuildSignedOffBy,
            cbsd_proto::BuildDestImage,
            cbsd_proto::BuildComponent,
            cbsd_proto::BuildTarget,
            cbsd_proto::BuildDescriptor,
            cbsd_proto::WorkerToken,
            // cbsd-server shared
            ErrorDetail,
            BuildRecord,
            BuildListRecord,
            ComponentInfo,
        )
    ),
    security(("bearer_auth" = [])),
    modifiers(&SecurityAddon),
    tags(
        (name = "auth",        description = "Authentication, tokens, and API keys"),
        (name = "builds",      description = "Build submission, listing, logs, and revocation"),
        (name = "workers",     description = "Worker listing"),
        (name = "admin",       description = "User management and worker registration"),
        (name = "permissions", description = "Roles and user-role assignments"),
        (name = "channels",    description = "Build channels and channel types"),
        (name = "periodic",    description = "Periodic build tasks"),
        (name = "components",  description = "Available build components"),
    )
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("PASETO / API key (cbsk_…)")
                    .build(),
            ),
        );
    }
}

/// Assemble the full OpenAPI spec by merging all route-module fragments.
pub fn build_openapi() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    doc.merge(crate::routes::auth::openapi());
    doc.merge(crate::routes::builds::openapi());
    doc.merge(crate::routes::workers::openapi());
    doc.merge(crate::routes::admin::openapi());
    doc.merge(crate::routes::permissions::openapi());
    doc.merge(crate::routes::channels::openapi());
    doc.merge(crate::routes::periodic::openapi());
    doc.merge(crate::routes::components::openapi());
    doc
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Serve the raw OpenAPI JSON spec at `GET /api/docs/openapi.json`.
async fn openapi_json() -> impl IntoResponse {
    let spec = build_openapi();
    let json = serde_json::to_string_pretty(&spec).unwrap_or_default();
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        json,
    )
}

/// Serve the Swagger UI at `GET /api/docs`.
///
/// Loads Swagger UI assets from the official CDN and points it at the local
/// `/api/docs/openapi.json` endpoint.
async fn swagger_ui_html() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html>
<head>
  <title>CBS API — Swagger UI</title>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
<div id="swagger-ui"></div>
<script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
<script>
  SwaggerUIBundle({
    url: "/api/docs/openapi.json",
    dom_id: '#swagger-ui',
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
    layout: "BaseLayout",
    deepLinking: true,
    persistAuthorization: true,
  })
</script>
</body>
</html>"#,
    )
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Return a `Router<()>` serving the Swagger UI and OpenAPI spec.
/// Intended to be merged into the top-level router after `.with_state()`.
pub fn router() -> Router {
    Router::new()
        .route("/api/docs", get(swagger_ui_html))
        .route("/api/docs/openapi.json", get(openapi_json))
}
