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
//! resolves credentials. [`SecretsMgr::git_url_for`] resolves a git URL (plain,
//! no-match, or vault-backed); [`SecretsMgr::s3_creds`] resolves S3 credentials
//! by exact key. Vault-backed entries read their `ces-kv` `key` through the
//! [`Vault`](crate::utils::vault) client (C4a). The signing/registry families
//! resolve later, with their consumers.

use std::collections::BTreeMap;

use crate::utils::subprocess::CommandError;
use crate::utils::vault::{Vault, VaultError};

pub mod git;
pub mod mgr;
pub mod storage;
pub mod utils;

pub use git::GitUrl;
pub use mgr::SecretsMgr;

/// An error resolving a secret.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// The git URL did not parse as a supported git URL.
    #[error("invalid git url '{0}'")]
    InvalidUrl(String),
    /// No storage secret is configured for the requested key.
    #[error("storage secret '{0}' not found")]
    StorageSecretNotFound(String),
    /// A vault-backed secret matched, but no Vault client is configured.
    #[error("secret requires vault, but no vault is configured")]
    VaultRequired,
    /// Reading the vault-backed secret failed.
    #[error("error reading secret from vault")]
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

/// Read the `ces-kv` secret at `path`, erroring if a matched vault-backed secret
/// has no Vault configured (the `SecretsMgr` carries the client). Shared by the
/// git and storage resolvers.
pub(crate) async fn read_vault_secret(
    vault: Option<&Vault>,
    path: &str,
) -> Result<BTreeMap<String, String>, SecretsError> {
    let vault = vault.ok_or(SecretsError::VaultRequired)?;
    Ok(vault.read_secret(path).await?)
}

/// Pull `field` from a vault secret, trailing-whitespace-trimmed (`.rstrip()`);
/// a missing field is an error.
pub(crate) fn vault_field(
    secret: &BTreeMap<String, String>,
    field: &str,
) -> Result<String, SecretsError> {
    secret
        .get(field)
        .map(|v| v.trim_end().to_string())
        .ok_or_else(|| SecretsError::MissingVaultField {
            field: field.to_string(),
        })
}
