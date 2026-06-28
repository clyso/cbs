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

//! The HashiCorp Vault client (design 004). Source: `cbscore/utils/vault.py`.
//!
//! [`Vault`] authenticates with one of three backends — selected in the order
//! **AppRole → userpass → token** (invariant 8) — and reads KV v2 secrets from
//! the pinned **`ces-kv`** mount. Vault-backed secret entries
//! ([`crate::types::GitSecret::VaultSsh`], `VaultS3`, …) resolve their value by
//! reading their `key` through [`Vault::read_secret`].
//!
//! `vaultrs` replaces Python's `hvac`. Like Python, a fresh client is built and
//! authenticated on every call (no token caching — a Python `FIXME` reproduced;
//! see `ROADMAP`). TLS trust follows `vaultrs`'s reqwest/rustls backend
//! (webpki roots plus the standard `VAULT_CACERT` / `VAULT_CAPATH` for a private
//! CA), the Vault-native mechanism — see the design 004 fidelity note.

use std::collections::BTreeMap;

use cbscore_types::VaultConfig;
use vaultrs::client::{Client as _, VaultClient, VaultClientSettingsBuilder};
use vaultrs::error::ClientError;

/// The KV v2 mount every secret is read from (`vault.py:61`), pinned.
const CES_KV_MOUNT: &str = "ces-kv";

/// An error talking to Vault. No secret value is ever included (002/004).
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// The vault config sets no authentication method (`vault.py:184`).
    #[error("no authentication method configured for vault")]
    NoAuthMethod,
    /// A configured authentication method has an empty credential field
    /// (`vault.py:93-96`/`125-128`/`154-156`).
    #[error("missing vault credential '{0}'")]
    MissingCredential(&'static str),
    /// The vault address is missing or not an `http(s)` URL (`vault.py:48`).
    #[error("invalid vault address '{0}' (expected an http(s) URL)")]
    InvalidAddr(String),
    /// Vault returned 403 on login or read (`hvac.exceptions.Forbidden`).
    #[error("permission denied accessing vault")]
    PermissionDenied,
    /// Building the client settings failed.
    #[error("error building vault client settings: {0}")]
    Settings(String),
    /// Authenticating against the backend failed (non-403).
    #[error("error logging in to vault")]
    Login(#[source] ClientError),
    /// Reading the secret failed (non-403): transport, decode, or missing entry.
    #[error("error obtaining secret from vault")]
    Read(#[source] ClientError),
}

/// The selected authentication backend, mirroring `vault.py`'s three `Vault`
/// subclasses. Carries no `Debug`-leaking trait beyond the field privacy of the
/// enclosing module; the credentials never reach a log line.
enum VaultAuth {
    AppRole { role_id: String, secret_id: String },
    UserPass { username: String, password: String },
    Token(String),
}

/// A configured Vault endpoint. Holds the address and the chosen auth backend;
/// each [`Vault::read_secret`] builds and authenticates a fresh client.
pub struct Vault {
    addr: String,
    auth: VaultAuth,
}

impl Vault {
    /// Build a [`Vault`] from a [`VaultConfig`], picking the first configured
    /// backend in the order **AppRole → userpass → token** (`vault.py:165-184`).
    pub fn from_config(config: &VaultConfig) -> Result<Self, VaultError> {
        let addr = config.vault_addr.clone();
        // Validate by parsing with the same parser `VaultClientSettingsBuilder::
        // address` uses (it `.unwrap()`s) — once this is `Ok` with an http(s)
        // scheme, building the client cannot panic on a hand-authored address.
        match url::Url::parse(&addr) {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => {}
            _ => return Err(VaultError::InvalidAddr(addr)),
        }

        let auth = if let Some(approle) = &config.auth_approle {
            VaultAuth::AppRole {
                role_id: require(&approle.role_id, "role-id")?,
                secret_id: require(&approle.secret_id, "secret-id")?,
            }
        } else if let Some(user) = &config.auth_user {
            VaultAuth::UserPass {
                username: require(&user.username, "username")?,
                password: require(&user.password, "password")?,
            }
        } else if let Some(token) = &config.auth_token {
            VaultAuth::Token(require(token, "auth-token")?)
        } else {
            return Err(VaultError::NoAuthMethod);
        };

        Ok(Self { addr, auth })
    }

    /// Read the KV v2 secret at `path` under the `ces-kv` mount and return its
    /// `data.data` map (`vault.py:56-75`). A fresh authenticated client is used.
    pub async fn read_secret(&self, path: &str) -> Result<BTreeMap<String, String>, VaultError> {
        let client = self.client().await?;
        vaultrs::kv2::read::<BTreeMap<String, String>>(&client, CES_KV_MOUNT, path)
            .await
            .map_err(|e| match forbidden(&e) {
                true => VaultError::PermissionDenied,
                false => VaultError::Read(e),
            })
    }

    /// Verify Vault is reachable and the configured backend authenticates,
    /// without reading a secret (`vault.py:77-84`). Used at `SecretsMgr`
    /// construction.
    pub async fn check_connection(&self) -> Result<(), VaultError> {
        self.client().await.map(|_| ())
    }

    /// Build a client and authenticate it with the configured backend. For
    /// token auth the token is set directly; for AppRole/userpass we log in and
    /// install the returned client token (`vault.py:100-162`).
    async fn client(&self) -> Result<VaultClient, VaultError> {
        let mut builder = VaultClientSettingsBuilder::default();
        builder.address(&self.addr);
        if let VaultAuth::Token(token) = &self.auth {
            builder.token(token);
        }
        let settings = builder
            .build()
            .map_err(|e| VaultError::Settings(e.to_string()))?;
        let mut client = VaultClient::new(settings).map_err(VaultError::Login)?;

        match &self.auth {
            VaultAuth::AppRole { role_id, secret_id } => {
                let auth = vaultrs::auth::approle::login(&client, "approle", role_id, secret_id)
                    .await
                    .map_err(login_err)?;
                client.set_token(&auth.client_token);
            }
            VaultAuth::UserPass { username, password } => {
                let auth = vaultrs::auth::userpass::login(&client, "userpass", username, password)
                    .await
                    .map_err(login_err)?;
                client.set_token(&auth.client_token);
            }
            VaultAuth::Token(_) => {}
        }

        Ok(client)
    }
}

/// Return `value` (owned) if non-empty, else a [`VaultError::MissingCredential`]
/// — validated at construction, as Python's backend `__init__`s do
/// (`vault.py:93-96`/`125-128`/`154-156`), not deferred to a login failure.
fn require(value: &str, field: &'static str) -> Result<String, VaultError> {
    if value.is_empty() {
        Err(VaultError::MissingCredential(field))
    } else {
        Ok(value.to_string())
    }
}

/// Whether a `vaultrs` error is a 403 (Vault's permission denied), mapped to
/// [`VaultError::PermissionDenied`] like Python's `hvac.exceptions.Forbidden`.
fn forbidden(err: &ClientError) -> bool {
    matches!(err, ClientError::APIError { code: 403, .. })
}

/// Map a login error: 403 → permission denied, else a wrapped login error.
fn login_err(err: ClientError) -> VaultError {
    if forbidden(&err) {
        VaultError::PermissionDenied
    } else {
        VaultError::Login(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbscore_types::{VaultAppRole, VaultUserPass};

    fn config(
        addr: &str,
        approle: Option<VaultAppRole>,
        user: Option<VaultUserPass>,
        token: Option<&str>,
    ) -> VaultConfig {
        VaultConfig {
            schema_version: 1,
            vault_addr: addr.to_string(),
            auth_user: user,
            auth_approle: approle,
            auth_token: token.map(str::to_string),
        }
    }

    #[test]
    fn auth_order_prefers_approle_over_userpass_and_token() {
        // All three configured: AppRole wins (vault.py:165-184).
        let cfg = config(
            "https://vault.example:8200",
            Some(VaultAppRole {
                role_id: "rid".to_string(),
                secret_id: "sid".to_string(),
            }),
            Some(VaultUserPass {
                username: "u".to_string(),
                password: "p".to_string(),
            }),
            Some("tok"),
        );
        let vault = Vault::from_config(&cfg).unwrap();
        assert!(matches!(vault.auth, VaultAuth::AppRole { .. }));
    }

    #[test]
    fn auth_order_falls_back_to_userpass_then_token() {
        let cfg = config(
            "https://vault.example:8200",
            None,
            Some(VaultUserPass {
                username: "u".to_string(),
                password: "p".to_string(),
            }),
            Some("tok"),
        );
        assert!(matches!(
            Vault::from_config(&cfg).unwrap().auth,
            VaultAuth::UserPass { .. }
        ));

        let cfg = config("https://vault.example:8200", None, None, Some("tok"));
        assert!(matches!(
            Vault::from_config(&cfg).unwrap().auth,
            VaultAuth::Token(_)
        ));
    }

    #[test]
    fn no_auth_method_is_an_error() {
        let cfg = config("https://vault.example:8200", None, None, None);
        assert!(matches!(
            Vault::from_config(&cfg),
            Err(VaultError::NoAuthMethod)
        ));
    }

    #[test]
    fn an_empty_credential_field_is_rejected_at_construction() {
        // An empty secret-id (or any credential) errors up front, not at a later
        // login (vault.py validates in the backend __init__).
        let cfg = config(
            "https://vault.example:8200",
            Some(VaultAppRole {
                role_id: "rid".to_string(),
                secret_id: String::new(),
            }),
            None,
            None,
        );
        assert!(matches!(
            Vault::from_config(&cfg),
            Err(VaultError::MissingCredential("secret-id"))
        ));

        // An empty token is likewise rejected.
        let cfg = config("https://vault.example:8200", None, None, Some(""));
        assert!(matches!(
            Vault::from_config(&cfg),
            Err(VaultError::MissingCredential("auth-token"))
        ));
    }

    #[test]
    fn a_malformed_address_is_rejected_not_panicked() {
        // Each of these would panic in `VaultClientSettingsBuilder::address`
        // (it `Url::parse(...).unwrap()`s); `from_config` must reject them
        // cleanly instead (the parse happens up front).
        for addr in [
            "vault.example:8200",  // no scheme
            "ftp://vault.example", // wrong scheme
            "http://",             // empty host
            "http://host:notaport",
            "not a url",
        ] {
            let cfg = config(addr, None, None, Some("tok"));
            assert!(
                matches!(Vault::from_config(&cfg), Err(VaultError::InvalidAddr(_))),
                "expected InvalidAddr for '{addr}'"
            );
        }
    }

    /// End-to-end read against a live Vault. Seeds a `ces-kv` secret with the
    /// dev-root token (`vaultrs` directly), then reads it back through our
    /// token-auth [`Vault`]. Ignored — needs a running dev server:
    ///
    /// ```text
    /// vault server -dev -dev-root-token-id=root
    /// export VAULT_ADDR=http://127.0.0.1:8200 VAULT_TOKEN=root
    /// vault secrets enable -path=ces-kv kv-v2      # one-time
    /// cargo test -p cbscore --lib -- --ignored vault_read_secret
    /// ```
    #[tokio::test]
    #[ignore = "requires a live Vault dev server with a ces-kv kv-v2 mount"]
    async fn vault_read_secret_round_trips_a_ces_kv_secret() {
        let addr = std::env::var("VAULT_ADDR").expect("VAULT_ADDR set for the dev server");
        let token = std::env::var("VAULT_TOKEN").expect("VAULT_TOKEN set to the dev root token");

        // Seed a secret directly (our Vault only reads).
        let settings = VaultClientSettingsBuilder::default()
            .address(&addr)
            .token(&token)
            .build()
            .expect("seed client settings");
        let seed_client = VaultClient::new(settings).expect("seed client");
        let want = BTreeMap::from([
            ("username".to_string(), "git".to_string()),
            ("password".to_string(), "s3cr3t".to_string()),
        ]);
        vaultrs::kv2::set(&seed_client, CES_KV_MOUNT, "ci/git", &want)
            .await
            .expect("seed ces-kv/ci/git");

        // Read it back through our token-auth Vault.
        let cfg = config(&addr, None, None, Some(&token));
        let vault = Vault::from_config(&cfg).expect("vault from config");
        let got = vault.read_secret("ci/git").await.expect("read secret");
        assert_eq!(got, want);
    }
}
