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

//! The `VersionType` enum (design 002). The `label → (type, description)` table
//! and the `get_version_type` lookup are version-domain data and live in
//! `cbscore::versions` (design 006), where they are consumed.

use serde::{Deserialize, Serialize};

/// The kind of version being created. Wire values are lowercase, matching the
/// Python `StrEnum` (`release`/`dev`/`test`/`ci`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionType {
    Release,
    Dev,
    Test,
    Ci,
}

impl VersionType {
    /// The lowercase wire/label string — also the per-type subdirectory under
    /// the version store (`<store>/<type>/…`).
    pub fn as_str(&self) -> &'static str {
        match self {
            VersionType::Release => "release",
            VersionType::Dev => "dev",
            VersionType::Test => "test",
            VersionType::Ci => "ci",
        }
    }
}

impl std::fmt::Display for VersionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::VersionType;

    #[test]
    fn wire_values_are_lowercase() {
        // The serde representation is the on-disk descriptor's `<type>` and the
        // store subdirectory name; pin it.
        assert_eq!(
            serde_json::to_string(&VersionType::Release).unwrap(),
            "\"release\""
        );
        assert_eq!(VersionType::Ci.as_str(), "ci");
        assert_eq!(
            serde_json::from_str::<VersionType>("\"dev\"").unwrap(),
            VersionType::Dev
        );
    }
}
