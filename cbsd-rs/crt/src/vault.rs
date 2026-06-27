// crt — Vault signing-key fetch (edge shim).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fetch the OpenPGP signing key from HashiCorp Vault (KV v2) at seal time
//! (design §6.1). A thin edge shim: it hands the armored key bytes to
//! `crt-core`, which does the actual signing — `crt-core` never touches Vault
//! or the network. The key is never persisted by `crt`.
//!
//! Authentication mirrors cbscore's three methods — a pre-issued token,
//! username/password (userpass), or AppRole — selected by [`VaultSecrets::auth`].
//! The vault secret holds the armored private key under a `private-key` field
//! (plus an optional `passphrase`), matching cbscore's `GPGVaultPrivateKeySecret`
//! convention so a single Vault secret can serve both tools.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use vaultrs::client::{Client, VaultClient, VaultClientSettingsBuilder};

use crate::secrets::{VaultAuth, VaultSecrets};

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
/// `private-key` field (and optionally `passphrase`). The client authenticates
/// with whichever method [`VaultSecrets::auth`] resolves (token / userpass /
/// AppRole).
pub async fn fetch_signing_key(vault: &VaultSecrets) -> Result<SigningKey> {
    let full_path = vault
        .keys
        .get(SIGNING_KEY_NAME)
        .with_context(|| format!("secrets `vault.keys` has no {SIGNING_KEY_NAME:?} entry"))?;
    let (mount, secret_path) = split_kv2_path(full_path)?;

    let auth = vault.auth()?;

    // Only token auth seeds the client token up front. userpass and AppRole
    // start token-less and exchange their credentials for a token below. When
    // `.token()` is unset the builder seeds it from `$VAULT_TOKEN` or empty, so
    // `.build()` always succeeds; `set_token` then overwrites it after login.
    let mut builder = VaultClientSettingsBuilder::default();
    builder.address(vault.addr.clone());
    if let VaultAuth::Token(token) = auth {
        builder.token(token.to_owned());
    }
    let settings = builder
        .build()
        .context("building the Vault client settings")?;
    let mut client = VaultClient::new(settings).context("creating the Vault client")?;

    match auth {
        VaultAuth::Token(_) => {}
        VaultAuth::UserPass(up) => {
            let info =
                vaultrs::auth::userpass::login(&client, &up.mount, &up.username, &up.password)
                    .await
                    .context("authenticating to Vault with userpass")?;
            client.set_token(&info.client_token);
        }
        VaultAuth::AppRole(ar) => {
            let info =
                vaultrs::auth::approle::login(&client, &ar.mount, &ar.role_id, &ar.secret_id)
                    .await
                    .context("authenticating to Vault with AppRole")?;
            client.set_token(&info.client_token);
        }
    }

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
    use crate::secrets::{VaultAppRole, VaultUserPass};
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

    /// A `VaultSecrets` pointing the signing key at `CRT_TEST_VAULT_PATH` on the
    /// `CRT_TEST_VAULT_ADDR` server, with no auth method set (the caller fills
    /// one in). Shared by the live tests below.
    fn live_vault_base() -> VaultSecrets {
        let mut keys = BTreeMap::new();
        keys.insert(
            SIGNING_KEY_NAME.to_owned(),
            std::env::var("CRT_TEST_VAULT_PATH").expect("CRT_TEST_VAULT_PATH"),
        );
        VaultSecrets {
            addr: std::env::var("CRT_TEST_VAULT_ADDR").expect("CRT_TEST_VAULT_ADDR"),
            token: None,
            userpass: None,
            approle: None,
            keys,
        }
    }

    fn assert_is_private_key(key: &SigningKey) {
        assert!(key.armored_private_key.contains("BEGIN PGP PRIVATE KEY"));
    }

    /// Live token-auth fetch. Opt-in: set `CRT_TEST_VAULT_ADDR`,
    /// `CRT_TEST_VAULT_TOKEN`, and `CRT_TEST_VAULT_PATH`, then run with
    /// `cargo test -p crt -- --ignored`. Never runs in plain `cargo test`.
    #[tokio::test]
    #[ignore = "requires a live Vault; set CRT_TEST_VAULT_* and run --ignored"]
    async fn fetch_signing_key_token_real() {
        let vault = VaultSecrets {
            token: Some(std::env::var("CRT_TEST_VAULT_TOKEN").expect("CRT_TEST_VAULT_TOKEN")),
            ..live_vault_base()
        };
        assert_is_private_key(&fetch_signing_key(&vault).await.unwrap());
    }

    /// Live userpass-auth fetch. Adds `CRT_TEST_VAULT_USERNAME` /
    /// `CRT_TEST_VAULT_PASSWORD` (and optional `CRT_TEST_VAULT_USERPASS_MOUNT`).
    #[tokio::test]
    #[ignore = "requires a live Vault; set CRT_TEST_VAULT_* and run --ignored"]
    async fn fetch_signing_key_userpass_real() {
        let userpass = VaultUserPass {
            username: std::env::var("CRT_TEST_VAULT_USERNAME").expect("CRT_TEST_VAULT_USERNAME"),
            password: std::env::var("CRT_TEST_VAULT_PASSWORD").expect("CRT_TEST_VAULT_PASSWORD"),
            mount: std::env::var("CRT_TEST_VAULT_USERPASS_MOUNT")
                .unwrap_or_else(|_| "userpass".to_owned()),
        };
        let vault = VaultSecrets {
            userpass: Some(userpass),
            ..live_vault_base()
        };
        assert_is_private_key(&fetch_signing_key(&vault).await.unwrap());
    }

    /// Live AppRole-auth fetch. Adds `CRT_TEST_VAULT_ROLE_ID` /
    /// `CRT_TEST_VAULT_SECRET_ID` (and optional `CRT_TEST_VAULT_APPROLE_MOUNT`).
    #[tokio::test]
    #[ignore = "requires a live Vault; set CRT_TEST_VAULT_* and run --ignored"]
    async fn fetch_signing_key_approle_real() {
        let approle = VaultAppRole {
            role_id: std::env::var("CRT_TEST_VAULT_ROLE_ID").expect("CRT_TEST_VAULT_ROLE_ID"),
            secret_id: std::env::var("CRT_TEST_VAULT_SECRET_ID").expect("CRT_TEST_VAULT_SECRET_ID"),
            mount: std::env::var("CRT_TEST_VAULT_APPROLE_MOUNT")
                .unwrap_or_else(|_| "approle".to_owned()),
        };
        let vault = VaultSecrets {
            approle: Some(approle),
            ..live_vault_base()
        };
        assert_is_private_key(&fetch_signing_key(&vault).await.unwrap());
    }
}
