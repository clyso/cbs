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

//! Version validation and the new auto-UUIDv7 resolution (design 006).

use uuid::Uuid;

use crate::types::Error;
use crate::versions::parse::parse_version;

/// Whether `s` is a valid UUID **version 7** (the sortable, time-ordered kind
/// `versions create` generates).
fn is_uuid_v7(s: &str) -> bool {
    Uuid::parse_str(s)
        .map(|u| u.get_version() == Some(uuid::Version::SortRand))
        .unwrap_or(false)
}

/// Validate the version a human supplied: it must be either a Python-shape
/// version string with **both** minor and patch present (matching Python's
/// `_validate_version`) **or** a valid UUIDv7. Anything else is
/// [`Error::MalformedVersion`].
pub fn validate_version(version: &str) -> Result<(), Error> {
    if let Ok(p) = parse_version(version)
        && p.minor.is_some()
        && p.patch.is_some()
    {
        return Ok(());
    }
    if is_uuid_v7(version) {
        return Ok(());
    }
    Err(Error::MalformedVersion(format!(
        "version '{version}' must be at least <major>.<minor>.<patch> or a UUIDv7"
    )))
}

/// Resolve the `VERSION` positional: a supplied value is used as-is; an omitted
/// one auto-generates a sortable **UUIDv7** (new in the port, design 006).
pub fn resolve_version(cli: Option<&str>) -> String {
    match cli {
        Some(v) => v.to_string(),
        None => Uuid::now_v7().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_mmp_and_uuidv7_rejects_others() {
        assert!(validate_version("20.2.1").is_ok());
        assert!(validate_version("ces-v20.2.1-rc.1").is_ok());
        // A freshly generated UUIDv7 validates.
        let v7 = resolve_version(None);
        assert!(validate_version(&v7).is_ok());

        assert!(validate_version("19").is_err());
        assert!(validate_version("19.2").is_err()); // minor but no patch
        assert!(validate_version("foobar").is_err());
        // A v4 UUID is not accepted (only v7).
        assert!(validate_version("936da01f-9abd-4d9d-80c7-02af85c822a8").is_err());
    }

    #[test]
    fn resolve_supplied_is_passthrough_omitted_is_uuidv7() {
        assert_eq!(resolve_version(Some("20.2.1")), "20.2.1");
        let generated = resolve_version(None);
        let parsed = Uuid::parse_str(&generated).expect("generated a valid UUID");
        assert_eq!(parsed.get_version(), Some(uuid::Version::SortRand));
    }
}
