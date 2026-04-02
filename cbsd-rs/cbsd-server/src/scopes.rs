// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

//! Shared scope pattern matching used by all enforcement points.

/// Match a scope pattern against a value.
///
/// - `*` matches everything (global wildcard).
/// - `prefix/*` matches any value starting with `prefix/`.
/// - Otherwise, exact match.
pub fn scope_pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
}

/// Check whether a channel scope pattern grants visibility to a channel.
///
/// A user with scope `ces-devel/dev` should see the `ces-devel` channel,
/// even though `scope_pattern_matches("ces-devel/dev", "ces-devel")` is
/// false. This function checks whether the pattern *covers* any type
/// under the given channel name.
///
/// Returns true when:
/// - `pattern == "*"` (global wildcard)
/// - `pattern` starts with `"{channel_name}/"` (e.g. `ces-devel/dev`)
/// - `pattern == "{channel_name}/*"` (channel wildcard)
pub fn scope_covers_channel(pattern: &str, channel_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let prefix = format!("{channel_name}/");
    pattern.starts_with(&prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- scope_pattern_matches ------------------------------------------------

    #[test]
    fn pattern_exact_match() {
        assert!(scope_pattern_matches("ces/dev", "ces/dev"));
        assert!(!scope_pattern_matches("ces/dev", "ces/release"));
    }

    #[test]
    fn pattern_wildcard_suffix() {
        assert!(scope_pattern_matches("ces/*", "ces/dev"));
        assert!(scope_pattern_matches("ces/*", "ces/release"));
        assert!(!scope_pattern_matches("ces/*", "ccs/dev"));
    }

    #[test]
    fn pattern_global_wildcard() {
        assert!(scope_pattern_matches("*", "ces/dev"));
        assert!(scope_pattern_matches("*", "anything"));
    }

    #[test]
    fn pattern_no_false_prefix_match() {
        // "ces-devel/*" must NOT match the raw channel name "ces-devel"
        // (the bug that motivated consolidating this function).
        assert!(!scope_pattern_matches("ces-devel/*", "ces-devel"));
    }

    #[test]
    fn pattern_empty_value() {
        assert!(scope_pattern_matches("*", ""));
        assert!(!scope_pattern_matches("ces/*", ""));
    }

    // -- scope_covers_channel -------------------------------------------------

    #[test]
    fn covers_global_wildcard() {
        assert!(scope_covers_channel("*", "ces-devel"));
        assert!(scope_covers_channel("*", "anything"));
    }

    #[test]
    fn covers_specific_type() {
        assert!(scope_covers_channel("ces-devel/dev", "ces-devel"));
        assert!(scope_covers_channel("ces-devel/release", "ces-devel"));
    }

    #[test]
    fn covers_channel_wildcard() {
        assert!(scope_covers_channel("ces-devel/*", "ces-devel"));
    }

    #[test]
    fn covers_no_match_different_channel() {
        assert!(!scope_covers_channel("ces-prod/dev", "ces-devel"));
        assert!(!scope_covers_channel("ces-prod/*", "ces-devel"));
    }

    #[test]
    fn covers_no_match_exact_channel_name() {
        // A pattern that IS the channel name (no slash) does not grant
        // visibility — scope patterns for channels always use channel/type.
        assert!(!scope_covers_channel("ces-devel", "ces-devel"));
    }
}
