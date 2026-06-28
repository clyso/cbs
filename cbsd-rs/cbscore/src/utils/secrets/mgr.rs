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

//! The secrets manager (design 004). Source: `cbscore/utils/secrets/mgr.py`.
//!
//! [`SecretsMgr`] wraps the merged [`Secrets`] plus an optional [`Vault`] client
//! and exposes resolution. [`SecretsMgr::git_url_for`] resolves plain, no-match,
//! and (with a Vault configured) vault-backed git secrets; the
//! storage/signing/registry resolvers land with their consumers.

use camino::Utf8PathBuf;

use crate::types::Secrets;
use crate::utils::secrets::SecretsError;
use crate::utils::secrets::git::{GitUrl, git_url_for};
use crate::utils::vault::Vault;

/// Resolves secrets for a build. Wraps the merged [`Secrets`] and an optional
/// [`Vault`] client (present when the config declares a vault). The SSH
/// directory — where `git_url_for` materialises a plain/vault SSH key — defaults
/// to `$HOME/.ssh` (Python's `Path.home()/.ssh`) and is injectable so tests can
/// point it at a tempdir without mutating the environment.
pub struct SecretsMgr {
    secrets: Secrets,
    ssh_dir: Utf8PathBuf,
    vault: Option<Vault>,
}

impl SecretsMgr {
    /// Wrap `secrets` with no Vault, materialising any plain-SSH key under
    /// `$HOME/.ssh`. Vault-backed secrets error until a Vault is supplied.
    pub fn new(secrets: Secrets) -> Self {
        Self {
            secrets,
            ssh_dir: default_ssh_dir(),
            vault: None,
        }
    }

    /// Wrap `secrets` with an explicit SSH directory and no Vault (used by
    /// tests).
    pub fn with_ssh_dir(secrets: Secrets, ssh_dir: Utf8PathBuf) -> Self {
        Self {
            secrets,
            ssh_dir,
            vault: None,
        }
    }

    /// Wrap `secrets` with a [`Vault`] client for vault-backed resolution
    /// (`mgr.py`: the manager carries the vault). The caller verifies the
    /// connection (via [`Vault::check_connection`]) before constructing.
    pub fn with_vault(secrets: Secrets, vault: Option<Vault>) -> Self {
        Self {
            secrets,
            ssh_dir: default_ssh_dir(),
            vault,
        }
    }

    /// Resolve a git clone URL against the configured git secrets — plain,
    /// no-match, or (with a Vault configured) vault-backed. Keep the returned
    /// [`GitUrl`] alive until the clone completes — for an SSH secret it owns the
    /// temporary key's cleanup guard.
    pub async fn git_url_for(&self, url: &str) -> Result<GitUrl, SecretsError> {
        git_url_for(url, &self.secrets.git, &self.ssh_dir, self.vault.as_ref()).await
    }
}

/// `$HOME/.ssh`, read once at construction (`HOME` empty → a relative `.ssh`).
fn default_ssh_dir() -> Utf8PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Utf8PathBuf::from(home).join(".ssh")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use camino::Utf8Path;

    use crate::types::GitSecret;

    fn secrets_with_git(git: BTreeMap<String, GitSecret>) -> Secrets {
        Secrets {
            schema_version: 1,
            git,
            storage: BTreeMap::new(),
            sign: BTreeMap::new(),
            registry: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn git_url_for_delegates_to_the_configured_git_secrets() {
        let git = BTreeMap::from([(
            "github.com".to_string(),
            GitSecret::PlainHttps {
                username: "git".to_string(),
                password: "s3cr3t".to_string(),
            },
        )]);
        let ssh = tempfile::tempdir().unwrap();
        let mgr = SecretsMgr::with_ssh_dir(
            secrets_with_git(git),
            Utf8Path::from_path(ssh.path()).unwrap().to_owned(),
        );

        let resolved = mgr
            .git_url_for("https://github.com/ceph/ceph")
            .await
            .unwrap();
        assert_eq!(
            resolved.arg().plaintext(),
            "https://git:s3cr3t@github.com/ceph/ceph"
        );
        assert!(!resolved.arg().redacted().contains("s3cr3t"));
    }

    #[test]
    fn default_ssh_dir_is_under_home() {
        // Constructed without an explicit dir, the manager targets `$HOME/.ssh`.
        let mgr = SecretsMgr::new(secrets_with_git(BTreeMap::new()));
        assert!(mgr.ssh_dir.as_str().ends_with(".ssh"), "{}", mgr.ssh_dir);
    }
}
