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

//! OpenAPI spec assembly: security schemes, metadata, and doc routes.

use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use utoipa::openapi::OpenApi;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi as OpenApiDerive};

use utoipa_scalar::Servable;

use crate::app::AppState;

/// Top-level OpenAPI metadata. Paths and schemas are collected
/// automatically by `OpenApiRouter` — this struct only provides
/// API-level info and security scheme definitions.
#[derive(OpenApiDerive)]
#[openapi(
    info(
        title = "CBS Build Service",
        description = "REST API for the CES Build System daemon"
    ),
    modifiers(&SecurityAddon)
)]
struct ApiDoc;

/// Registers the two authentication schemes used by the API.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);

        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("PASETO")
                    .build(),
            ),
        );

        components.add_security_scheme(
            "cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("id"))),
        );
    }
}

/// Apply `ApiDoc` metadata (info, security schemes) to the collected spec.
fn apply_spec_metadata(spec: &mut OpenApi) {
    let base = ApiDoc::openapi();

    // Apply metadata from ApiDoc to the spec.
    spec.info = base.info;
    spec.info.version = env!("CARGO_PKG_VERSION").to_string();

    if let Some(base_components) = base.components {
        let components = spec.components.get_or_insert_with(Default::default);
        for (name, scheme) in base_components.security_schemes {
            components.add_security_scheme(name, scheme);
        }
    }
}

/// Mount the Scalar UI and JSON spec endpoint onto a router.
pub fn doc_routes(mut openapi: OpenApi) -> Router<AppState> {
    apply_spec_metadata(&mut openapi);

    let json_spec = Arc::new(openapi.clone());
    let scalar: Router<()> = utoipa_scalar::Scalar::with_url("/docs", openapi).into();

    Router::<AppState>::new()
        .merge(scalar.with_state(()))
        .route(
            "/docs/openapi.json",
            get(move || {
                let s = json_spec.clone();
                async move { Json(s) }
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_serializes_to_valid_json() {
        let spec = ApiDoc::openapi();
        let json = spec
            .to_pretty_json()
            .expect("OpenAPI spec must serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("serialized spec must be valid JSON");
        assert!(parsed.get("openapi").is_some(), "must have 'openapi' key");
        assert!(parsed.get("info").is_some(), "must have 'info' key");
    }
}
