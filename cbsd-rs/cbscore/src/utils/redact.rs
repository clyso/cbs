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

//! Secret redaction (design 003), type-driven first and pattern-driven as a
//! backstop. A secret is wrapped in a type that exposes no formatting trait, so
//! it is structurally impossible to log the plaintext by formatting the
//! argument (invariant 4); a string-pattern pass additionally censors
//! credential-bearing flags that arrive as plain strings.
//!
//! C1 lands [`SecureArg`], [`CmdArg`], [`Password`], and the pattern backstop —
//! enough to establish and test the compiler-enforced redaction invariant. The
//! `PasswordArg` (credential flag) and `SecureUrl` (credentialed git URL) types
//! land with their first consumers (C3/C4/C5).

use std::fmt;
use std::sync::{Arc, LazyLock};

use regex::Regex;

/// The marker emitted in place of a secret value.
const CENSORED: &str = "<CENSORED>";

/// A command argument that carries a secret.
///
/// Deliberately has **no** `Debug`/`Display` supertrait, so a secret cannot be
/// `{:?}`/`{}`-formatted directly — there is no `#[derive(Debug)]` that could
/// leak it, and the compiler enforces this rather than review discipline
/// (invariant 4). The plaintext is reachable only through [`plaintext`], used
/// only when spawning a process.
///
/// [`plaintext`]: SecureArg::plaintext
pub trait SecureArg: Send + Sync {
    /// The plaintext value — used **only** when spawning a process.
    fn plaintext(&self) -> String;
    /// The censored rendering — used for logs and errors.
    fn redacted(&self) -> String;
}

/// A single command argument: a plain string or a secret.
#[derive(Clone)]
pub enum CmdArg {
    Plain(String),
    Secure(Arc<dyn SecureArg>),
}

impl CmdArg {
    /// The plaintext rendering, used **only** at spawn.
    pub fn plaintext(&self) -> String {
        match self {
            CmdArg::Plain(s) => s.clone(),
            CmdArg::Secure(a) => a.plaintext(),
        }
    }

    /// The redacted rendering, used for logs and errors. A plain argument still
    /// gets the inline credential-flag backstop applied.
    pub fn redacted(&self) -> String {
        match self {
            CmdArg::Plain(s) => redact_inline(s),
            CmdArg::Secure(a) => a.redacted(),
        }
    }
}

// Both formatting traits emit the redacted form, so `format!("{:?}", arg)` and
// `format!("{}", arg)` can never leak a secret.
impl fmt::Debug for CmdArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

impl fmt::Display for CmdArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

impl From<&str> for CmdArg {
    fn from(s: &str) -> Self {
        CmdArg::Plain(s.to_string())
    }
}

impl From<String> for CmdArg {
    fn from(s: String) -> Self {
        CmdArg::Plain(s)
    }
}

/// A password value (the simplest secret). Does **not** implement `Debug` or
/// `Display`, so it cannot leak via formatting; reach its value only through
/// [`SecureArg::plaintext`].
pub struct Password(String);

impl Password {
    pub fn new(value: impl Into<String>) -> Self {
        Password(value.into())
    }
}

impl SecureArg for Password {
    fn plaintext(&self) -> String {
        self.0.clone()
    }

    fn redacted(&self) -> String {
        CENSORED.to_string()
    }
}

/// Credential-bearing flags whose plain-string values are censored in logs.
/// Broader than Python (which covers only `--pass`/`--passphrase`); this only
/// affects what is logged, never what is spawned, so broadening it is safe
/// (design 003).
const CREDENTIAL_FLAGS: &[&str] = &["--passphrase", "--password", "--pass", "-p"];

/// Inline `--flag=value` credential censoring for a single plain argument.
fn redact_inline(s: &str) -> String {
    static INLINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(--passphrase|--password|--pass|-p)=.+$")
            .expect("inline credential regex is valid")
    });
    match INLINE.captures(s) {
        Some(caps) => format!("{}={CENSORED}", &caps[1]),
        None => s.to_string(),
    }
}

/// The redacted rendering of a whole command line: typed secrets are censored
/// via [`SecureArg::redacted`], and plain credential flags are censored both
/// inline (`--flag=value`) and two-token (`--flag value`).
pub fn sanitize_cmdline(args: &[CmdArg]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut next_secure = false;
    for arg in args {
        match arg {
            CmdArg::Secure(a) => {
                out.push(a.redacted());
                next_secure = false;
            }
            CmdArg::Plain(s) => {
                if next_secure {
                    out.push(CENSORED.to_string());
                    next_secure = false;
                } else if CREDENTIAL_FLAGS.contains(&s.as_str()) {
                    next_secure = true;
                    out.push(s.clone());
                } else {
                    out.push(redact_inline(s));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(value: &str) -> CmdArg {
        CmdArg::Secure(Arc::new(Password::new(value)))
    }

    #[test]
    fn typed_secret_never_leaks_via_formatting() {
        let arg = secret("hunter2");
        // Both formatting traits redact; the plaintext only escapes via plaintext().
        assert_eq!(format!("{arg:?}"), CENSORED);
        assert_eq!(format!("{arg}"), CENSORED);
        assert_eq!(arg.redacted(), CENSORED);
        assert!(!format!("{arg:?}").contains("hunter2"));
        assert_eq!(arg.plaintext(), "hunter2");
    }

    #[test]
    fn pattern_backstop_censors_credential_flags() {
        let cmd: Vec<CmdArg> = [
            "buildah",
            "push",
            "--password",
            "s3cr3t",              // two-token value
            "--pass=inlinesecret", // inline value
            "-p=9000",             // -p inline (broadened)
            "image",
        ]
        .iter()
        .map(|s| CmdArg::from(*s))
        .collect();

        let sanitized = sanitize_cmdline(&cmd);
        let joined = sanitized.join(" ");
        assert!(
            !joined.contains("s3cr3t"),
            "two-token value leaked: {joined}"
        );
        assert!(
            !joined.contains("inlinesecret"),
            "inline value leaked: {joined}"
        );
        assert_eq!(sanitized[2], "--password");
        assert_eq!(sanitized[3], CENSORED);
        assert_eq!(sanitized[4], format!("--pass={CENSORED}"));
        assert_eq!(sanitized[5], format!("-p={CENSORED}"));
    }

    #[test]
    fn secure_arg_in_cmdline_is_redacted() {
        let cmd = vec![CmdArg::from("git"), CmdArg::from("clone"), secret("tok")];
        let sanitized = sanitize_cmdline(&cmd);
        assert_eq!(sanitized[2], CENSORED);
        assert!(!sanitized.join(" ").contains("tok"));
    }
}
