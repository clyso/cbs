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

//! Storage (S3) credential resolution (design 004). Source:
//! `cbscore/utils/secrets/storage.py`.

use std::collections::BTreeMap;

use crate::types::StorageSecret;
use crate::utils::secrets::{SecretsError, read_vault_secret, vault_field};
use crate::utils::vault::Vault;

/// Resolve `(hostname, access_id, secret_id)` for the S3 endpoint whose key is
/// exactly `url` (`storage.py:25-56`). Storage secrets are looked up by **exact
/// key**, not longest-prefix. A `plain-s3` secret returns its literal
/// credentials; a `vault-s3` secret reads its `ces-kv` `key` and pulls the named
/// `access-id`/`secret-id` fields (trailing-whitespace-trimmed). The returned
/// hostname is the lookup `url` itself, as in Python.
pub(crate) async fn s3_creds(
    storage: &BTreeMap<String, StorageSecret>,
    vault: Option<&Vault>,
    url: &str,
) -> Result<(String, String, String), SecretsError> {
    let entry = storage
        .get(url)
        .ok_or_else(|| SecretsError::StorageSecretNotFound(url.to_string()))?;
    match entry {
        StorageSecret::PlainS3 {
            access_id,
            secret_id,
        } => Ok((url.to_string(), access_id.clone(), secret_id.clone())),
        StorageSecret::VaultS3 {
            key,
            access_id,
            secret_id,
        } => {
            let secret = read_vault_secret(vault, key).await?;
            let access_id = vault_field(&secret, access_id)?;
            let secret_id = vault_field(&secret, secret_id)?;
            Ok((url.to_string(), access_id, secret_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(entries: &[(&str, StorageSecret)]) -> BTreeMap<String, StorageSecret> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[tokio::test]
    async fn plain_s3_resolves_by_exact_key() {
        let map = storage(&[(
            "https://s3.example.com",
            StorageSecret::PlainS3 {
                access_id: "AKID".to_string(),
                secret_id: "SECRET".to_string(),
            },
        )]);
        let (host, access, secret) = s3_creds(&map, None, "https://s3.example.com")
            .await
            .unwrap();
        assert_eq!(host, "https://s3.example.com");
        assert_eq!(access, "AKID");
        assert_eq!(secret, "SECRET");
    }

    #[tokio::test]
    async fn lookup_is_exact_not_prefix() {
        // A prefix of the configured key must not match (unlike git's
        // longest-prefix lookup, storage is exact).
        let map = storage(&[(
            "https://s3.example.com/bucket",
            StorageSecret::PlainS3 {
                access_id: "AKID".to_string(),
                secret_id: "SECRET".to_string(),
            },
        )]);
        let err = s3_creds(&map, None, "https://s3.example.com")
            .await
            .unwrap_err();
        assert!(
            matches!(err, SecretsError::StorageSecretNotFound(_)),
            "{err}"
        );
    }

    #[tokio::test]
    async fn vault_s3_without_a_vault_is_an_error() {
        let map = storage(&[(
            "https://s3.example.com",
            StorageSecret::VaultS3 {
                key: "storage/s3".to_string(),
                access_id: "access-id".to_string(),
                secret_id: "secret-id".to_string(),
            },
        )]);
        let err = s3_creds(&map, None, "https://s3.example.com")
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::VaultRequired), "{err}");
    }
}
