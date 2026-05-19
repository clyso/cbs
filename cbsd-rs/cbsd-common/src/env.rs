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

//! Environment variable helpers shared by `cbsd-server` and
//! `cbsd-worker`. Provides strict, canonical "truthy" parsing for
//! boolean environment variables such as `CBSD_DEV`.

/// Closed set of accepted truthy values for [`is_truthy`] and
/// [`is_truthy_env`], matched ASCII-case-insensitively. Per audit-rem
/// D1 (Phase 2): tightens the previous `!value.is_empty()` test so
/// `CBSD_DEV=0` or `CBSD_DEV=false` no longer silently enable dev mode.
const TRUTHY_VALUES: &[&str] = &["1", "true", "yes", "on"];

/// Returns `true` if `value` matches a canonical truthy string
/// (case-insensitive). Returns `false` for any other value — including
/// `"0"`, `"false"`, `"no"`, `"off"`, an empty string, or arbitrary
/// garbage. This is the pure predicate behind [`is_truthy_env`].
pub fn is_truthy(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    TRUTHY_VALUES.iter().any(|t| *t == lower)
}

/// Returns `true` if the named environment variable is set to a truthy
/// value per [`is_truthy`]. Returns `false` if the variable is unset,
/// set to a non-UTF-8 value, or set to a value that fails [`is_truthy`].
pub fn is_truthy_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| is_truthy(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_truthy_values() {
        for v in ["1", "true", "yes", "on"] {
            assert!(is_truthy(v), "{v:?} must be truthy");
        }
    }

    #[test]
    fn truthy_is_case_insensitive() {
        for v in ["TRUE", "True", "tRuE", "ON", "Yes"] {
            assert!(is_truthy(v), "{v:?} must be truthy");
        }
    }

    #[test]
    fn rejects_falsy_strings() {
        for v in [
            "", "0", "false", "no", "off", "anything", "true!", " yes", "yes ",
        ] {
            assert!(!is_truthy(v), "{v:?} must NOT be truthy");
        }
    }

    #[test]
    fn rejects_unset_env_var() {
        // Use a name we are certain is not set in the test environment.
        let name = "CBSD_COMMON_TEST_NEVER_SET_5b7c0e9a";
        // Defensive: clear it just in case.
        // Safety: env-var mutation in tests is process-global. We use a
        // unique name to avoid colliding with other tests.
        unsafe { std::env::remove_var(name) };
        assert!(!is_truthy_env(name));
    }
}
