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
//! resolves a git URL via [`SecretsMgr::git_url_for`]. Plain, no-match, and
//! Vault-backed git secrets (`vault-ssh`/`vault-https`) all resolve — the latter
//! by reading their `ces-kv` `key` through the [`Vault`](crate::utils::vault)
//! client (C4a). The storage/signing/registry families resolve later too.

use crate::utils::subprocess::CommandError;
use crate::utils::vault::VaultError;

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
    /// A vault-backed git secret matched, but no Vault client is configured.
    #[error("git secret requires vault, but no vault is configured")]
    VaultRequired,
    /// Reading the vault-backed git secret failed.
    #[error("error reading git secret from vault")]
    Vault(#[from] VaultError),
    /// The vault secret did not contain the referenced field.
    #[error("vault secret is missing the '{field}' field")]
    MissingVaultField { field: String },
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
