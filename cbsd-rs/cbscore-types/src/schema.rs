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

//! Schema-version machinery shared by the formats cbscore produces (design
//! 002). Each marker is a `u32` that defaults to 1 when absent and is rejected
//! when higher than the parser implements.

use crate::error::Error;

/// The serde default for a `schema_version` field: a marker-less file (every
/// file written before this port) parses as v1.
pub fn schema_v1() -> u32 {
    1
}

/// Validate a parsed marker against the maximum the parser implements. A higher
/// value can only mean a breaking change this build does not understand, so it
/// is a hard error (design 002), not a silent mis-parse. Run *after*
/// deserialization, which has already applied `absent → v1`.
pub fn ensure_schema_version(format: &'static str, found: u32, max: u32) -> Result<(), Error> {
    if found > max {
        return Err(Error::UnknownSchemaVersion { format, found, max });
    }
    Ok(())
}
