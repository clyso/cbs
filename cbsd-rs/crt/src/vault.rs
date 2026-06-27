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
//! The signing key is one of the named `vault.keys` entries (chosen by
//! `crt.config.yaml`'s `gpg_private_key`); the entry names which fields inside
//! the KV v2 secret hold the armored key and passphrase (defaults `private-key`
//! / `passphrase`), matching cbscore's `GPGVaultPrivateKeySecret` convention.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use vaultrs::client::{Client, VaultClient, VaultClientSettingsBuilder};

use crate::secrets::{VaultAuth, VaultKeyEntry, VaultSecrets};

/// The fetched signing material handed to `crt-core::sign_manifest`.
pub struct SigningKey {
    pub armored_private_key: String,
    pub passphrase: Option<String>,
}

/// Pull the armored key (and optional passphrase) out of a KV v2 secret's data
/// dict, using the field names configured on `entry`. An absent passphrase
/// field is `None`. Pure (no IO) so the field mapping is unit-testable.
fn extract_signing_key(data: &Map<String, Value>, entry: &VaultKeyEntry) -> Result<SigningKey> {
    let armored = data
        .get(&entry.private_key_field)
        .and_then(|v| v.as_str())
        .with_context(|| {
            format!(
                "field {:?} is missing or not a string",
                entry.private_key_field
            )
        })?;
    // A present-but-non-string passphrase is a misconfiguration, not "no
    // passphrase" — surface it like the key field rather than silently dropping.
    let passphrase = match data.get(&entry.passphrase_field) {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .with_context(|| format!("field {:?} is not a string", entry.passphrase_field))?
                .to_owned(),
        ),
    };
    Ok(SigningKey {
        armored_private_key: armored.to_owned(),
        passphrase,
    })
}

/// Fetch the signing key named `key_name` (a `vault.keys` entry) from Vault. The
/// entry's `path` locates the KV v2 secret and its `private_key_field` /
/// `passphrase_field` name the fields to read. The client authenticates with
/// whichever method [`VaultSecrets::auth`] resolves (token / userpass / AppRole).
pub async fn fetch_signing_key(vault: &VaultSecrets, key_name: &str) -> Result<SigningKey> {
    let entry = vault
        .keys
        .get(key_name)
        .with_context(|| format!("secrets `vault.keys` has no {key_name:?} entry"))?;
    let (mount, secret_path) = split_kv2_path(&entry.path)?;

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

    let data: Map<String, Value> = vaultrs::kv2::read(&client, &mount, &secret_path)
        .await
        .with_context(|| format!("reading the Vault secret at {:?}", entry.path))?;

    extract_signing_key(&data, entry)
        .with_context(|| format!("in the Vault secret at {:?}", entry.path))
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

    /// The `vault.keys` entry name the live tests register and fetch.
    const TEST_KEY_NAME: &str = "signing";

    /// A `VaultSecrets` whose `signing` key points at `CRT_TEST_VAULT_PATH` on
    /// the `CRT_TEST_VAULT_ADDR` server, with no auth method set (the caller
    /// fills one in). The KV field names default to `private-key` / `passphrase`
    /// but honor `CRT_TEST_VAULT_KEY_FIELD` / `CRT_TEST_VAULT_PASS_FIELD`.
    fn live_vault_base() -> VaultSecrets {
        let mut keys = BTreeMap::new();
        keys.insert(
            TEST_KEY_NAME.to_owned(),
            VaultKeyEntry {
                path: std::env::var("CRT_TEST_VAULT_PATH").expect("CRT_TEST_VAULT_PATH"),
                private_key_field: std::env::var("CRT_TEST_VAULT_KEY_FIELD")
                    .unwrap_or_else(|_| "private-key".to_owned()),
                passphrase_field: std::env::var("CRT_TEST_VAULT_PASS_FIELD")
                    .unwrap_or_else(|_| "passphrase".to_owned()),
            },
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
        assert_is_private_key(&fetch_signing_key(&vault, TEST_KEY_NAME).await.unwrap());
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
        assert_is_private_key(&fetch_signing_key(&vault, TEST_KEY_NAME).await.unwrap());
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
        assert_is_private_key(&fetch_signing_key(&vault, TEST_KEY_NAME).await.unwrap());
    }

    /// A `VaultKeyEntry` naming the given fields (the path is irrelevant to
    /// `extract_signing_key`).
    fn entry_with(private_key_field: &str, passphrase_field: &str) -> VaultKeyEntry {
        VaultKeyEntry {
            path: "ces-kv/gpg/pvt".to_owned(),
            private_key_field: private_key_field.to_owned(),
            passphrase_field: passphrase_field.to_owned(),
        }
    }

    #[test]
    fn extract_reads_the_configured_fields() {
        let data: Map<String, Value> =
            serde_json::from_str(r#"{"key":"ARMORED","passphrase":"pw"}"#).unwrap();
        let sk = extract_signing_key(&data, &entry_with("key", "passphrase")).unwrap();
        assert_eq!(sk.armored_private_key, "ARMORED");
        assert_eq!(sk.passphrase.as_deref(), Some("pw"));
    }

    #[test]
    fn extract_treats_an_absent_passphrase_as_none() {
        let data: Map<String, Value> = serde_json::from_str(r#"{"key":"ARMORED"}"#).unwrap();
        let sk = extract_signing_key(&data, &entry_with("key", "passphrase")).unwrap();
        assert_eq!(sk.armored_private_key, "ARMORED");
        assert!(sk.passphrase.is_none());
    }

    #[test]
    fn extract_errors_when_the_key_field_is_missing() {
        // The secret stores `private-key`, but the entry looks for `key`.
        let data: Map<String, Value> =
            serde_json::from_str(r#"{"private-key":"ARMORED"}"#).unwrap();
        assert!(extract_signing_key(&data, &entry_with("key", "passphrase")).is_err());
    }

    #[test]
    fn extract_errors_when_the_key_value_is_not_a_string() {
        let data: Map<String, Value> = serde_json::from_str(r#"{"key":123}"#).unwrap();
        assert!(extract_signing_key(&data, &entry_with("key", "passphrase")).is_err());
    }

    #[test]
    fn extract_errors_when_the_passphrase_value_is_not_a_string() {
        // Present but not a string is a misconfig, not "no passphrase".
        let data: Map<String, Value> =
            serde_json::from_str(r#"{"key":"ARMORED","passphrase":123}"#).unwrap();
        assert!(extract_signing_key(&data, &entry_with("key", "passphrase")).is_err());
    }
}
