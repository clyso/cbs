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

//! The version-type label table and lookups (design 006). The `VersionType`
//! enum itself lives in `cbscore-types` (design 002); this is the version-domain
//! data (`utils.py:33-38`).

use crate::types::{Error, VersionType};

/// The `label → (VersionType, description)` table (`utils.py:33-38`). The
/// description (not the type name) is what titles use, e.g. `release` →
/// "General Availability".
fn release_type_entry(label: &str) -> Option<(VersionType, &'static str)> {
    match label {
        "release" => Some((VersionType::Release, "General Availability")),
        "dev" => Some((VersionType::Dev, "Development")),
        "test" => Some((VersionType::Test, "Testing")),
        "ci" => Some((VersionType::Ci, "CI/CD")),
        _ => None,
    }
}

/// Look up a [`VersionType`] by its label (case-insensitive). This is a
/// name→type **lookup**, not a `regex` parse (resolves 000's M3). An unknown
/// name yields [`Error::VersionError`].
pub fn get_version_type(type_name: &str) -> Result<VersionType, Error> {
    release_type_entry(&type_name.to_lowercase())
        .map(|(t, _)| t)
        .ok_or_else(|| Error::VersionError(format!("unknown version type '{type_name}'")))
}

/// The human description for a version type (e.g. `Release` → "General
/// Availability"), used in version titles.
pub fn get_version_type_desc(version_type: VersionType) -> &'static str {
    release_type_entry(version_type.as_str())
        .map(|(_, desc)| desc)
        .expect("every VersionType has a table entry")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_label_case_insensitively() {
        assert_eq!(get_version_type("release").unwrap(), VersionType::Release);
        assert_eq!(get_version_type("DEV").unwrap(), VersionType::Dev);
        assert_eq!(get_version_type("Test").unwrap(), VersionType::Test);
        assert_eq!(get_version_type("ci").unwrap(), VersionType::Ci);
    }

    #[test]
    fn rejects_unknown_and_does_not_parse_versions() {
        assert!(get_version_type("staging").is_err());
        // A name→type lookup, not a version parser: a version string is "unknown".
        assert!(get_version_type("20.2.1").is_err());
    }

    #[test]
    fn descriptions_match_python() {
        assert_eq!(
            get_version_type_desc(VersionType::Release),
            "General Availability"
        );
        assert_eq!(get_version_type_desc(VersionType::Dev), "Development");
        assert_eq!(get_version_type_desc(VersionType::Test), "Testing");
        assert_eq!(get_version_type_desc(VersionType::Ci), "CI/CD");
    }
}
