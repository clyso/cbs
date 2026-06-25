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

//! The version-store path helper (design 006).

use camino::{Utf8Path, Utf8PathBuf};

use crate::version_type::VersionType;

/// Build the on-disk path of a version descriptor:
/// `<root>/<type>/<version>.json`.
///
/// This is the single source of truth for the store layout, so the M1 writer
/// (hardcoded `<git-root>/_versions` root) and the M5 configurable root
/// (`--versions-dir` / `paths.versions`) never duplicate the path construction —
/// protecting on-disk-layout parity (invariant 3).
pub fn descriptor_path(root: &Utf8Path, version_type: VersionType, version: &str) -> Utf8PathBuf {
    root.join(version_type.as_str())
        .join(format!("{version}.json"))
}

#[cfg(test)]
mod tests {
    use super::descriptor_path;
    use crate::version_type::VersionType;
    use camino::Utf8Path;

    #[test]
    fn lays_out_root_type_version_json() {
        let p = descriptor_path(Utf8Path::new("/repo/_versions"), VersionType::Dev, "20.2.1");
        assert_eq!(p, Utf8Path::new("/repo/_versions/dev/20.2.1.json"));
    }
}
