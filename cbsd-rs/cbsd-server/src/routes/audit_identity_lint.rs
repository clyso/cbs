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

//! Regression guard: route handlers reachable by robot callers must not
//! format `user.email` into actor-identity audit log lines — the synthetic
//! `robot+<name>@robots` email leaks implementation detail and hides the
//! actor identity from operators. Use [`AuthUser::display_identity`] so
//! humans appear as their email and robots as `robot:<name>`.
//!
//! The allowlist below names files whose handlers are **guaranteed
//! human-only** via the forbidden-cap strip in `load_authed_user`: robots
//! cannot hold caps like `permissions:manage`, `channels:manage`,
//! `workers:manage`, or `periodic:manage`, so every actor-log call site in
//! those files is reachable only by a human caller. Files outside the
//! allowlist must use `display_identity()` for every `tracing::` macro
//! actor position.

use std::fs;
use std::path::PathBuf;

/// Files whose every `user.email` occurrence in a `tracing::` macro is
/// guaranteed to be a human caller (see module docstring for rationale).
const HUMAN_ONLY_ROUTES: &[&str] = &[
    "admin.rs", // admin:* caps — robots cannot hold
    // Every handler in `auth.rs` that logs `user.email` rejects non-human
    // callers upfront: `/token/revoke` returns 400 for both `cbsk_` and
    // `cbrk_` bearers; `/tokens/revoke-all` and `/api-keys` reject
    // `is_robot` callers before any logging. This is enforced in code,
    // not merely by cap design.
    "auth.rs",
    "channels.rs",    // channels:manage cap — robots cannot hold
    "periodic.rs",    // periodic:* caps — robots cannot hold
    "permissions.rs", // permissions:manage cap — robots cannot hold
];

/// Route files exempt from the scan for reasons other than caller identity:
/// `mod.rs` has no handlers, `components.rs` logs nothing actor-relevant,
/// and `workers.rs` / `robots.rs` are manipulated only by admins — plus
/// `robots.rs` itself already routes through `display_identity()` everywhere.
const EXEMPT_ROUTES: &[&str] = &[
    "mod.rs",
    "audit_identity_lint.rs",
    "components.rs",
    "robots.rs",
    "workers.rs",
];

/// Return byte offsets of every `user.email` occurrence that falls inside
/// the argument list of a `tracing::info!` / `warn!` / `error!` macro in
/// the supplied source. The heuristic tracks paren depth; it is not a
/// full Rust parser but is robust enough for typical log call sites.
fn find_violations(src: &str) -> Vec<usize> {
    let mut violations: Vec<usize> = Vec::new();
    let mut in_macro = false;
    let mut paren_depth: i32 = 0;

    for (line_idx, line) in src.lines().enumerate() {
        if !in_macro
            && (line.contains("tracing::info!(")
                || line.contains("tracing::warn!(")
                || line.contains("tracing::error!("))
        {
            in_macro = true;
            paren_depth = 0;
        }

        if in_macro {
            for ch in line.chars() {
                if ch == '(' {
                    paren_depth += 1;
                } else if ch == ')' {
                    paren_depth -= 1;
                }
            }

            if line.contains("user.email") {
                violations.push(line_idx + 1);
            }

            if paren_depth <= 0 {
                in_macro = false;
            }
        }
    }

    violations
}

#[test]
fn actor_log_lines_in_robot_reachable_routes_use_display_identity() {
    let routes_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/routes");
    let entries = fs::read_dir(&routes_dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", routes_dir.display()));

    let mut offenders: Vec<String> = Vec::new();

    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".rs") {
            continue;
        }
        if HUMAN_ONLY_ROUTES.contains(&name) || EXEMPT_ROUTES.contains(&name) {
            continue;
        }

        let src =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for lineno in find_violations(&src) {
            offenders.push(format!("{name}:{lineno}"));
        }
    }

    assert!(
        offenders.is_empty(),
        "robot-reachable route handlers must use `user.display_identity()` in \
         `tracing::` actor-format positions, not `user.email`. Offenders: {offenders:?}. \
         If the handler is provably human-only via the forbidden-cap strip, add the \
         file to `HUMAN_ONLY_ROUTES` in this module with a one-line rationale."
    );
}

#[cfg(test)]
mod self_tests {
    use super::*;

    #[test]
    fn find_violations_flags_user_email_inside_tracing_info() {
        let src = r#"
fn a() {
    tracing::info!("user {} did thing", user.email);
}
"#;
        assert_eq!(find_violations(src), vec![3]);
    }

    #[test]
    fn find_violations_ignores_user_email_outside_log_macros() {
        let src = r#"
fn a(user: AuthUser) {
    let e = user.email;
    tracing::info!("neutral");
    let x = something_else(user.email).await?;
}
"#;
        assert!(find_violations(src).is_empty());
    }

    #[test]
    fn find_violations_handles_multi_line_macros() {
        let src = r#"
fn a() {
    tracing::info!(
        "user {} did thing",
        user.email,
    );
}
"#;
        assert_eq!(find_violations(src), vec![5]);
    }
}
