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

//! Secret resolution (design 004). The format types (`Secrets`, `GitSecret`, …)
//! live in `cbscore-types`; this module is the resolution layer over them. Its
//! first consumer is component preparation (C3), which needs a git clone URL
//! with credentials folded in.
//!
//! [`SecretsMgr`] wraps the merged [`Secrets`](cbscore_types::Secrets) and
//! resolves a git URL via [`SecretsMgr::git_url_for`]. **This slice resolves the
//! plain and no-match cases only** — `plain-ssh`/`plain-https`/`plain-token` and
//! "no configured secret". Vault-backed git secrets (`vault-ssh`/`vault-https`)
//! return [`SecretsError::VaultUnimplemented`] and complete in C4a, when the
//! Vault client lands. The storage/signing/registry families resolve later too.

use crate::utils::subprocess::CommandError;

pub mod git;
pub mod mgr;
pub mod utils;

pub use git::GitUrl;
pub use mgr::SecretsMgr;

/// An error resolving a secret.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// The git URL did not parse as a supported git URL.
    #[error("invalid git url '{0}'")]
    InvalidUrl(String),
    /// A vault-backed git secret matched, but the Vault client is not yet
    /// implemented (lands in C4a).
    #[error("vault-backed git secret resolution is not yet implemented (C4a)")]
    VaultUnimplemented,
    /// `ssh-keyscan` could not be spawned (or timed out).
    #[error("error running ssh-keyscan for host '{host}'")]
    Keyscan {
        host: String,
        #[source]
        source: CommandError,
    },
    /// `ssh-keyscan` exited non-zero or returned no host key.
    #[error("could not obtain ssh host key for '{host}': {stderr}")]
    KeyscanFailed { host: String, stderr: String },
    /// A filesystem operation materialising the SSH config/key failed.
    #[error("ssh secret io error ({context})")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}
