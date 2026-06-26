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

//! CES Build System core build library — the Rust port of Python `cbscore`.
//!
//! `cbscore` is the build-and-release engine that `cbsbuild` (the CLI) and
//! `cbsd-worker` drive. It owns subprocess execution, the shell-tool wrappers,
//! configuration and secrets, the S3 and Vault clients, the builder pipeline,
//! and the two-phase runner (design 001).
//!
//! This is the C0 scaffold: it establishes the crate and its facade over the
//! zero-IO type layer. The subsystems land by capability in later commits
//! (design 001 commit map), each with the dependencies its code first uses.

/// The zero-IO type layer, re-exported so consumers reach the wire types and
/// constants through a single `cbscore::types::…` path.
pub use cbscore_types as types;

/// The `cbscore` library version (`CARGO_PKG_VERSION`).
///
/// `cbsbuild` reports this next to its own version so an operator can see which
/// library build the CLI was linked against.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod components;
pub mod config;
pub mod images;
pub mod utils;
pub mod versions;
