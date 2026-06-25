// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! `VersionDescriptor` and its sub-types (design 002) — the JSON `versions
//! create` produces and the runner/builder read. Source of truth:
//! `cbscore/versions/desc.py`. Field names are snake_case with no `rename_all`;
//! the on-disk shape equals Python's `model_dump_json` plus the new
//! `schema_version` marker.

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::schema::{ensure_schema_version, schema_v1};

/// The git user that created the version (`signed_off_by` block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSignedOffBy {
    pub user: String,
    pub email: String,
}

/// The version descriptor's own image coordinates (`image` block). Distinct
/// from the external `ImageDescriptor` (design 006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionImage {
    pub registry: String,
    pub name: String,
    pub tag: String,
}

/// One component pinned in a version: name, repo URL, and git ref. The wire key
/// for the ref is `ref` (a Rust keyword), renamed back on serialisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionComponent {
    pub name: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
}

/// The version descriptor written by `versions create` to
/// `<store>/<type>/<version>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionDescriptor {
    /// Schema marker; absent on read → 1 (see [`crate::schema`]).
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    pub version: String,
    pub title: String,
    pub signed_off_by: VersionSignedOffBy,
    pub image: VersionImage,
    pub components: Vec<VersionComponent>,
    pub distro: String,
    pub el_version: u32,
}

impl VersionDescriptor {
    const SCHEMA_FORMAT: &'static str = "version descriptor";

    /// The highest `schema_version` this build understands.
    pub const SCHEMA_MAX: u32 = 1;

    /// Parse a descriptor from JSON, applying the schema-version policy: an
    /// absent marker is v1, a marker `<= SCHEMA_MAX` is accepted, and a higher
    /// one is rejected with [`Error::UnknownSchemaVersion`]. Pure — no IO; the
    /// optional `path` is carried into [`Error::InvalidVersionDescriptor`] only
    /// for operator context (the IO reader passes the file path, a pure caller
    /// passes `None`).
    pub fn parse(json: &str, path: Option<&camino::Utf8Path>) -> Result<Self, Error> {
        let desc: VersionDescriptor =
            serde_json::from_str(json).map_err(|_| Error::InvalidVersionDescriptor {
                path: path.map(ToOwned::to_owned),
            })?;
        ensure_schema_version(Self::SCHEMA_FORMAT, desc.schema_version, Self::SCHEMA_MAX)?;
        Ok(desc)
    }

    /// Serialise to the pretty (2-space) JSON operators read, matching Python's
    /// `model_dump_json(indent=2)` shape.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> VersionDescriptor {
        VersionDescriptor {
            schema_version: 1,
            version: "20.2.1".to_string(),
            title: "Release General Availability version 20.2.1".to_string(),
            signed_off_by: VersionSignedOffBy {
                user: "Jane Doe".to_string(),
                email: "jane@example.com".to_string(),
            },
            image: VersionImage {
                registry: "harbor.clyso.com".to_string(),
                name: "ces/ceph/ceph".to_string(),
                tag: "20.2.1".to_string(),
            },
            components: vec![VersionComponent {
                name: "ceph".to_string(),
                repo: "https://github.com/ceph/ceph".to_string(),
                git_ref: "v20.2.1".to_string(),
            }],
            distro: "rockylinux:9".to_string(),
            el_version: 9,
        }
    }

    #[test]
    fn round_trips() {
        let d = sample();
        let json = d.to_json_pretty().unwrap();
        let back = VersionDescriptor::parse(&json, None).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn component_ref_uses_wire_key() {
        let json = sample().to_json_pretty().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        // The wire key is `ref`, not `git_ref`.
        assert_eq!(value["components"][0]["ref"], "v20.2.1");
        assert!(value["components"][0].get("git_ref").is_none());
    }

    #[test]
    fn marker_absent_parses_as_v1() {
        // A descriptor written before this port (no `schema_version`) must parse.
        let json = r#"{
            "version": "20.2.1",
            "title": "t",
            "signed_off_by": {"user": "u", "email": "e"},
            "image": {"registry": "r", "name": "n", "tag": "t"},
            "components": [],
            "distro": "rockylinux:9",
            "el_version": 9
        }"#;
        let d = VersionDescriptor::parse(json, None).unwrap();
        assert_eq!(d.schema_version, 1);
    }

    #[test]
    fn higher_marker_is_rejected() {
        let json = r#"{
            "schema_version": 2,
            "version": "20.2.1",
            "title": "t",
            "signed_off_by": {"user": "u", "email": "e"},
            "image": {"registry": "r", "name": "n", "tag": "t"},
            "components": [],
            "distro": "rockylinux:9",
            "el_version": 9
        }"#;
        let err = VersionDescriptor::parse(json, None).unwrap_err();
        assert!(matches!(
            err,
            Error::UnknownSchemaVersion {
                found: 2,
                max: 1,
                ..
            }
        ));
    }

    #[test]
    fn malformed_json_carries_the_path() {
        let path = camino::Utf8Path::new("/store/dev/x.json");
        let err = VersionDescriptor::parse("{ not json", Some(path)).unwrap_err();
        match err {
            Error::InvalidVersionDescriptor { path: Some(p) } => assert_eq!(p, path),
            other => panic!("expected InvalidVersionDescriptor with path, got {other:?}"),
        }
    }
}
