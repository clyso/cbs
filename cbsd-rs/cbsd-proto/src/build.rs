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

use serde::{Deserialize, Serialize};

use crate::arch::Arch;

/// Opaque build identifier. Maps to SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BuildId(pub i64);

impl std::fmt::Display for BuildId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Build priority. Strict precedence: high before normal before low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Priority {
    High,
    #[default]
    Normal,
    Low,
}

/// Build lifecycle state. Lowercase in DB and API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildState {
    Queued,
    Dispatched,
    Started,
    Revoking,
    Success,
    Failure,
    Revoked,
}

impl std::fmt::Display for BuildState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Dispatched => write!(f, "dispatched"),
            Self::Started => write!(f, "started"),
            Self::Revoking => write!(f, "revoking"),
            Self::Success => write!(f, "success"),
            Self::Failure => write!(f, "failure"),
            Self::Revoked => write!(f, "revoked"),
        }
    }
}

/// Version type. Matches Python `cbscore.versions.utils.VersionType`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionType {
    Release,
    Dev,
    Test,
    Ci,
}

/// Signed-off-by field. Server overwrites this from the `users` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSignedOffBy {
    pub user: String,
    pub email: String,
}

/// Destination container image name and tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDestImage {
    pub name: String,
    pub tag: String,
}

/// A component to be built. The `repo` field is an optional override URL;
/// if absent, the component's default repo from `cbs.component.yaml` is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildComponent {
    pub name: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// Build target environment. Nested inside `BuildDescriptor.build`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildTarget {
    pub distro: String,
    pub os_version: String,
    #[serde(default = "default_artifact_type")]
    pub artifact_type: String,
    #[serde(default = "default_arch")]
    pub arch: Arch,
}

fn default_artifact_type() -> String {
    "rpm".to_string()
}

fn default_arch() -> Arch {
    Arch::X86_64
}

/// Describes a build to the build service. Preserves the Python
/// `cbsdcore.versions.BuildDescriptor` nesting for JSON compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDescriptor {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_type: Option<VersionType>,
    pub signed_off_by: BuildSignedOffBy,
    pub dst_image: BuildDestImage,
    pub components: Vec<BuildComponent>,
    pub build: BuildTarget,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_descriptor() -> BuildDescriptor {
        BuildDescriptor {
            version: "19.2.3".to_string(),
            channel: Some("ces-devel".to_string()),
            version_type: Some(VersionType::Dev),
            signed_off_by: BuildSignedOffBy {
                user: "Alice".to_string(),
                email: "alice@clyso.com".to_string(),
            },
            dst_image: BuildDestImage {
                name: "harbor.clyso.com/ces-devel/ceph".to_string(),
                tag: "v19.2.3-dev.1".to_string(),
            },
            components: vec![BuildComponent {
                name: "ceph".to_string(),
                git_ref: "v19.2.3".to_string(),
                repo: Some("https://github.com/clyso/ceph".to_string()),
            }],
            build: BuildTarget {
                distro: "rockylinux".to_string(),
                os_version: "el9".to_string(),
                artifact_type: "rpm".to_string(),
                arch: Arch::Aarch64,
            },
        }
    }

    #[test]
    fn serde_round_trip() {
        let desc = sample_descriptor();
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: BuildDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(desc, parsed);
    }

    #[test]
    fn deserialize_with_arm64_alias() {
        let json = r#"{
            "version": "19.2.3",
            "channel": "ces",
            "version_type": "release",
            "signed_off_by": {"user": "Bob", "email": "bob@clyso.com"},
            "dst_image": {"name": "harbor.clyso.com/ces/ceph", "tag": "v19.2.3"},
            "components": [{"name": "ceph", "ref": "v19.2.3"}],
            "build": {"distro": "rockylinux", "os_version": "el9", "arch": "arm64"}
        }"#;
        let desc: BuildDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(desc.build.arch, Arch::Aarch64);
        assert_eq!(desc.build.artifact_type, "rpm");
    }

    #[test]
    fn priority_default() {
        assert_eq!(Priority::default(), Priority::Normal);
    }

    #[test]
    fn build_state_display() {
        assert_eq!(BuildState::Queued.to_string(), "queued");
        assert_eq!(BuildState::Revoking.to_string(), "revoking");
        assert_eq!(BuildState::Success.to_string(), "success");
    }

    #[test]
    fn build_id_display() {
        assert_eq!(BuildId(42).to_string(), "42");
    }
}
