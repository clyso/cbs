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

//! The build artifact report (design 002).
//!
//! Source: `cbscore/builder/report.py`. The in-container `Builder` writes this
//! as `build-report.json` to the scratch mount; the host runner reads it back
//! across that mount (before the return-code check, design 009), then it
//! propagates to the worker and server.
//!
//! Unlike the config/secrets formats, the report is **JSON, snake_case**, and
//! its optionals **serialize as `null`** when unset (no `skip_serializing_if`)
//! — matching pydantic's `model_dump_json`, since the worker and `cbsd-server`
//! already consume this exact shape. The marker keeps its existing name,
//! `report_version` (converging on `schema_version` is roadmapped; design 001).

use serde::{Deserialize, Serialize};

/// The serde default for `report_version`: an absent marker is version 1.
fn report_v1() -> u32 {
    1
}

/// Summary of the artifacts a build produced (design 002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildArtifactReport {
    #[serde(default = "report_v1")]
    pub report_version: u32,
    pub version: String,
    pub skipped: bool,
    pub container_image: Option<ContainerImageReport>,
    pub release_descriptor: Option<ReleaseDescriptorReport>,
    #[serde(default)]
    pub components: Vec<ComponentReport>,
}

impl BuildArtifactReport {
    /// The highest `report_version` this build understands.
    pub const REPORT_MAX: u32 = 1;
    /// Human-facing format name for marker errors.
    pub const SCHEMA_FORMAT: &'static str = "build report";
}

/// The container image a build produced (populated on both the skipped and
/// full-build paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerImageReport {
    pub name: String,
    pub tag: String,
    pub pushed: bool,
}

/// Where the release descriptor landed in S3 (`None` on the skipped path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseDescriptorReport {
    pub s3_path: String,
    pub bucket: String,
}

/// A single component included in the build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentReport {
    pub name: String,
    pub version: String,
    pub sha1: String,
    pub repo_url: String,
    /// S3 path to the RPM artifacts; `null` when not uploaded.
    pub rpms_s3_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_report_round_trips() {
        let report = BuildArtifactReport {
            report_version: 1,
            version: "20.2.1".to_string(),
            skipped: false,
            container_image: Some(ContainerImageReport {
                name: "harbor.clyso.com/ces/ceph".to_string(),
                tag: "20.2.1".to_string(),
                pushed: true,
            }),
            release_descriptor: Some(ReleaseDescriptorReport {
                s3_path: "releases/20.2.1.json".to_string(),
                bucket: "cbs-releases".to_string(),
            }),
            components: vec![ComponentReport {
                name: "ceph".to_string(),
                version: "20.2.1-42.g5a0b003".to_string(),
                sha1: "5a0b003".to_string(),
                repo_url: "https://github.com/ceph/ceph".to_string(),
                rpms_s3_path: Some("art/ceph/rpm-20.2.1".to_string()),
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: BuildArtifactReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn unset_optionals_serialize_as_null() {
        // The report's rule is the opposite of the config's: emit `null`, not
        // omit, so the existing worker/server consumers see the shape they
        // expect.
        let report = BuildArtifactReport {
            report_version: 1,
            version: "20.2.1".to_string(),
            skipped: true,
            container_image: None,
            release_descriptor: None,
            components: vec![],
        };
        let json = serde_json::to_value(&report).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.get("release_descriptor").unwrap().is_null());
        assert!(obj.get("container_image").unwrap().is_null());
        assert_eq!(json["components"], serde_json::json!([]));
    }

    #[test]
    fn absent_marker_and_components_default() {
        // A report without report_version or components still parses.
        let json = r#"{"version":"20.2.1","skipped":true,"container_image":null,"release_descriptor":null}"#;
        let report: BuildArtifactReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.report_version, 1);
        assert!(report.components.is_empty());
    }

    #[test]
    fn null_rpms_path_round_trips() {
        let report = BuildArtifactReport {
            report_version: 1,
            version: "v".to_string(),
            skipped: false,
            container_image: None,
            release_descriptor: None,
            components: vec![ComponentReport {
                name: "ceph".to_string(),
                version: "v".to_string(),
                sha1: "abc".to_string(),
                repo_url: "u".to_string(),
                rpms_s3_path: None,
            }],
        };
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["components"][0]["rpms_s3_path"].is_null());
        let back: BuildArtifactReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
    }
}
