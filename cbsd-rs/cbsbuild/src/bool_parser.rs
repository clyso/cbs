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

//! Click-equivalent `BOOL` coercion and the `CBS_DEBUG` resolution (design 010,
//! H3). A naive presence check would read the non-empty string `CBS_DEBUG=0` as
//! "set" and turn debug on, silently inverting the off case the runner emits;
//! this coercion makes `CBS_DEBUG=0` off.

/// Coerce a string to a bool the way Click's `BOOL` type does: case-insensitive
/// `1`/`true`/`t`/`yes`/`y`/`on` → `true`; `0`/`false`/`f`/`no`/`n`/`off` and the
/// empty string → `false`; anything else → `None`.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "f" | "no" | "n" | "off" | "" => Some(false),
        _ => None,
    }
}

/// Whether debug logging is on: the `-d/--debug` flag **or** a Click-truthy
/// `CBS_DEBUG`. In particular `CBS_DEBUG=0` is off — not a presence check.
pub fn debug_enabled(flag: bool) -> bool {
    flag || std::env::var("CBS_DEBUG")
        .ok()
        .and_then(|v| parse_bool(&v))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_bool_coercion() {
        for t in ["1", "true", "T", "Yes", "y", "on", "ON"] {
            assert_eq!(parse_bool(t), Some(true), "{t} should be true");
        }
        // CBS_DEBUG=0 is off (the H3 trap); so are the other falsey forms.
        for f in ["0", "false", "f", "no", "N", "off", ""] {
            assert_eq!(parse_bool(f), Some(false), "{f} should be false");
        }
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn flag_forces_debug_on() {
        // The flag wins regardless of the environment.
        assert!(debug_enabled(true));
    }
}
