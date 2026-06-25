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

//! The versions subsystem (design 006): the version-string grammar and parse
//! helpers, the `VersionType` label table and lookups, and version validation
//! (including the new auto-UUIDv7 path). The descriptor wire types live in
//! `cbscore-types` (design 002); these helpers need `regex`/`uuid`, so they
//! live in the library. Source of truth: `cbscore/versions/utils.py`.

pub mod create;
pub mod parse;
pub mod validate;
pub mod version_type;

pub use create::{CreateError, VersionSpec, create, version_create_helper, write_descriptor};
pub use parse::{
    ParsedVersion, get_major_version, get_minor_version, normalize_version, parse_component_refs,
    parse_version,
};
pub use validate::{resolve_version, validate_version};
pub use version_type::{get_version_type, get_version_type_desc};
