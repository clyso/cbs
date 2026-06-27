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

//! The lowest layer of the library (design 003): the async subprocess
//! primitive, the secret-redaction machinery, and the shell-tool wrappers built
//! on top. `subprocess` and `git` are lift-out candidates for a future
//! shared-primitives crate, so they carry their own tracing targets and import
//! only primitives.

pub mod git;
pub mod podman;
pub mod redact;
pub mod secrets;
pub mod subprocess;
pub mod uris;
