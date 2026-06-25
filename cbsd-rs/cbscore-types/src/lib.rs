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

//! Zero-IO type layer for the `cbscore` Rust port.
//!
//! `cbscore-types` is the single source of truth for the wire formats cbscore
//! produces (version/release descriptors, the build report), the schema-version
//! markers, the `VersionType` enum, the type-layer error taxonomy, and the
//! tracing-target constants (design 002). It performs no IO — no filesystem,
//! subprocess, async, cloud SDK, or `regex`.
//!
//! This is the C0 scaffold (design 001 commit map): it ships the tracing-target
//! constants every subsystem references. The wire types and their `serde` /
//! `uuid` dependencies land with their first consumer in later commits, so no
//! type carries an unused dependency edge.

/// Canonical `tracing` target strings for the cbscore subsystems.
///
/// Centralising the target hierarchy here (design 002) lets every subsystem and
/// the `cbsbuild` subscriber setup (design 010) reference one source instead of
/// duplicating string literals. Only the constants live here; the subscriber
/// configuration and the `CBS_DEBUG` → level mapping live in `cbsbuild`.
pub mod tracing_targets {
    /// Subprocess execution wrapper (`utils::subprocess`).
    pub const SUBPROCESS: &str = "cbscore::utils::subprocess";

    /// Git command wrapper (`utils::git`).
    pub const GIT: &str = "cbscore::utils::git";

    /// Host-side two-phase runner.
    pub const RUNNER: &str = "cbscore::runner";

    /// In-container builder pipeline.
    pub const BUILDER: &str = "cbscore::builder";
}

#[cfg(test)]
mod tests {
    use super::tracing_targets;

    #[test]
    fn tracing_targets_share_the_cbscore_namespace() {
        // The subscriber filters spans by these target prefixes; pin the
        // namespace so a typo cannot silently detach a subsystem from the
        // filter (design 002).
        assert_eq!(tracing_targets::SUBPROCESS, "cbscore::utils::subprocess");
        assert_eq!(tracing_targets::GIT, "cbscore::utils::git");
        assert_eq!(tracing_targets::RUNNER, "cbscore::runner");
        assert_eq!(tracing_targets::BUILDER, "cbscore::builder");

        for target in [
            tracing_targets::SUBPROCESS,
            tracing_targets::GIT,
            tracing_targets::RUNNER,
            tracing_targets::BUILDER,
        ] {
            assert!(target.starts_with("cbscore::"));
        }
    }
}
