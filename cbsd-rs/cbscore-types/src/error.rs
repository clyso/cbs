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

//! Type-layer error taxonomy (design 002).
//!
//! These are the pure parse/validation and schema errors triggered by the wire
//! types and version strings — no IO. IO-triggered errors (a descriptor file
//! that is absent, no image descriptor matching a version) are raised by the
//! subsystem that performs the IO and are defined there (`cbscore`), not here.

use camino::Utf8PathBuf;

/// A type-layer (pure) error: parse, validation, or schema-version failure.
///
/// Names mirror the Python originals (`MalformedVersionError`, `VersionError`,
/// `InvalidVersionDescriptorError`); `UnknownSchemaVersion` is new to the port.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A version string that does not match any accepted shape.
    #[error("malformed version: {0}")]
    MalformedVersion(String),

    /// A version-domain failure (e.g. an unknown version type name).
    #[error("version error: {0}")]
    VersionError(String),

    /// A descriptor whose bytes fail to parse or validate. The `path` is carried
    /// for operator context; the type itself performs no IO (a pure parse from a
    /// string supplies `None`, the IO reader supplies the file path).
    #[error(
        "invalid version descriptor{}",
        .path.as_ref().map(|p| format!(" at '{p}'")).unwrap_or_default()
    )]
    InvalidVersionDescriptor { path: Option<Utf8PathBuf> },

    /// A descriptor/report carrying a `schema_version` marker higher than the
    /// parser implements — a hard error, not a silent mis-parse (design 002).
    #[error("unknown schema version for {format}: found {found}, max supported is {max}")]
    UnknownSchemaVersion {
        format: &'static str,
        found: u32,
        max: u32,
    },
}
