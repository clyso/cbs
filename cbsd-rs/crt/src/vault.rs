// crt — Vault signing-key fetch (edge shim).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fetch the OpenPGP signing key from HashiCorp Vault (KV v2) at seal time
//! (design §6.1). A thin edge shim: it hands the armored key bytes to
//! `crt-core`, which does the actual signing — `crt-core` never touches Vault
//! or the network. The key is never persisted by `crt`.
//!
//! The vault secret holds the armored private key under a `private-key` field
//! (plus an optional `passphrase`), matching cbscore's `GPGVaultPrivateKeySecret`
//! convention so a single Vault secret can serve both tools.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};

use crate::secrets::VaultSecrets;

/// The logical name (in `secrets.vault.keys`) of the release signing key.
pub const SIGNING_KEY_NAME: &str = "gpg_signing_private";

/// A KV v2 secret carrying an armored OpenPGP private key.
#[derive(Debug, Deserialize)]
struct SigningKeySecret {
    #[serde(rename = "private-key")]
    private_key: String,
    #[serde(default)]
    passphrase: Option<String>,
}

/// The fetched signing material handed to `crt-core::sign_manifest`.
pub struct SigningKey {
    pub armored_private_key: String,
    pub passphrase: Option<String>,
}

/// Fetch the release signing key named [`SIGNING_KEY_NAME`] from Vault. The
/// vault path comes from `secrets.vault.keys`; the secret must expose a
/// `private-key` field (and optionally `passphrase`).
pub async fn fetch_signing_key(vault: &VaultSecrets) -> Result<SigningKey> {
    let full_path = vault
        .keys
        .get(SIGNING_KEY_NAME)
        .with_context(|| format!("secrets `vault.keys` has no {SIGNING_KEY_NAME:?} entry"))?;
    let (mount, secret_path) = split_kv2_path(full_path)?;

    let settings = VaultClientSettingsBuilder::default()
        .address(vault.addr.clone())
        .token(vault.token.clone())
        .build()
        .context("building the Vault client settings")?;
    let client = VaultClient::new(settings).context("creating the Vault client")?;

    let secret: SigningKeySecret = vaultrs::kv2::read(&client, &mount, &secret_path)
        .await
        .with_context(|| format!("reading the signing key from Vault at {full_path:?}"))?;

    Ok(SigningKey {
        armored_private_key: secret.private_key,
        passphrase: secret.passphrase,
    })
}

/// Split a KV v2 path into `(mount, path)`. The configured path may be written
/// either as the raw API path (`secret/data/crt/openpgp-signing-key`) or
/// without the `data` infix (`secret/crt/openpgp-signing-key`); a literal
/// `data` second segment is dropped because `vaultrs::kv2::read` re-inserts it.
fn split_kv2_path(full: &str) -> Result<(String, String)> {
    let mut segments = full.split('/').filter(|s| !s.is_empty());
    let mount = segments
        .next()
        .with_context(|| format!("empty Vault key path: {full:?}"))?
        .to_owned();
    let mut rest: Vec<&str> = segments.collect();
    if rest.first() == Some(&"data") {
        rest.remove(0);
    }
    if rest.is_empty() {
        bail!("Vault key path {full:?} has no secret path after the mount");
    }
    Ok((mount, rest.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn splits_a_kv2_data_path() {
        let (mount, path) = split_kv2_path("secret/data/crt/openpgp-signing-key").unwrap();
        assert_eq!(mount, "secret");
        assert_eq!(path, "crt/openpgp-signing-key");
    }

    #[test]
    fn splits_a_path_without_the_data_infix() {
        let (mount, path) = split_kv2_path("kv/crt/key").unwrap();
        assert_eq!(mount, "kv");
        assert_eq!(path, "crt/key");
    }

    #[test]
    fn rejects_a_mount_only_path() {
        assert!(split_kv2_path("secret").is_err());
        assert!(split_kv2_path("secret/data").is_err());
    }

    /// Live Vault fetch. Opt-in: set `CRT_TEST_VAULT_ADDR`, `CRT_TEST_VAULT_TOKEN`,
    /// and `CRT_TEST_VAULT_PATH`, then run with `cargo test -p crt -- --ignored`.
    /// Never runs in plain `cargo test` (needs a live Vault).
    #[tokio::test]
    #[ignore = "requires a live Vault; set CRT_TEST_VAULT_* and run --ignored"]
    async fn fetch_signing_key_real() {
        let mut keys = BTreeMap::new();
        keys.insert(
            SIGNING_KEY_NAME.to_owned(),
            std::env::var("CRT_TEST_VAULT_PATH").expect("CRT_TEST_VAULT_PATH"),
        );
        let vault = VaultSecrets {
            addr: std::env::var("CRT_TEST_VAULT_ADDR").expect("CRT_TEST_VAULT_ADDR"),
            token: std::env::var("CRT_TEST_VAULT_TOKEN").expect("CRT_TEST_VAULT_TOKEN"),
            keys,
        };
        let key = fetch_signing_key(&vault).await.unwrap();
        assert!(key.armored_private_key.contains("BEGIN PGP PRIVATE KEY"));
    }
}
