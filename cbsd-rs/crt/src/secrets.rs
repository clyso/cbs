// crt — secrets loading.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The git-ignored `crt.secrets.yaml` (design §9): S3 credentials and the Vault
//! address/token + the vault path(s) of the signing key. Kept separate from the
//! non-secret `crt.config.yaml`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Secrets {
    #[serde(default)]
    pub s3: Option<S3Secrets>,
    #[serde(default)]
    pub vault: Option<VaultSecrets>,
}

#[derive(Debug, Deserialize)]
pub struct S3Secrets {
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Vault connection + the named vault paths of the signing key(s) (design §9).
/// The key fetch itself is a thin edge shim in `crate::vault`; `crt-core` only
/// ever sees the resulting armored key bytes.
///
/// Authentication mirrors the three methods cbscore supports (token, userpass,
/// AppRole); set **exactly one** — see [`VaultSecrets::auth`].
#[derive(Debug, Deserialize)]
pub struct VaultSecrets {
    pub addr: String,
    /// Token auth: a pre-issued Vault token.
    #[serde(default)]
    pub token: Option<String>,
    /// Username/password (userpass) auth.
    #[serde(default)]
    pub userpass: Option<VaultUserPass>,
    /// AppRole auth (role-id / secret-id).
    #[serde(default)]
    pub approle: Option<VaultAppRole>,
    /// Named signing-key secrets. `crt.config.yaml`'s `gpg_private_key` selects
    /// one by name; see [`VaultKeyEntry`].
    #[serde(default)]
    pub keys: BTreeMap<String, VaultKeyEntry>,
}

/// A named signing-key secret: the KV v2 path, plus which fields inside that
/// secret hold the armored private key and (optionally) its passphrase. The
/// field names default to `private-key` / `passphrase` but can name any fields
/// (e.g. a secret shaped `{ "key": "…", "passphrase": "…" }` sets
/// `private_key_field: key`).
#[derive(Debug, Deserialize)]
pub struct VaultKeyEntry {
    pub path: String,
    #[serde(default = "default_private_key_field")]
    pub private_key_field: String,
    #[serde(default = "default_passphrase_field")]
    pub passphrase_field: String,
}

fn default_private_key_field() -> String {
    "private-key".to_owned()
}

fn default_passphrase_field() -> String {
    "passphrase".to_owned()
}

/// Username/password auth against a Vault `userpass` mount.
#[derive(Debug, Deserialize)]
pub struct VaultUserPass {
    pub username: String,
    pub password: String,
    /// The auth method's mount path (default `userpass`).
    #[serde(default = "default_userpass_mount")]
    pub mount: String,
}

/// AppRole auth against a Vault `approle` mount.
#[derive(Debug, Deserialize)]
pub struct VaultAppRole {
    pub role_id: String,
    pub secret_id: String,
    /// The auth method's mount path (default `approle`).
    #[serde(default = "default_approle_mount")]
    pub mount: String,
}

fn default_userpass_mount() -> String {
    "userpass".to_owned()
}

fn default_approle_mount() -> String {
    "approle".to_owned()
}

/// The single resolved Vault auth method (see [`VaultSecrets::auth`]). Holds
/// only borrows, so it is `Copy`.
#[derive(Debug, Clone, Copy)]
pub enum VaultAuth<'a> {
    Token(&'a str),
    UserPass(&'a VaultUserPass),
    AppRole(&'a VaultAppRole),
}

impl VaultSecrets {
    /// Resolve the configured auth method, requiring **exactly one** of
    /// `token`, `userpass`, or `approle`. Zero methods or more than one is an
    /// error — this is the single validation point both `seal` and
    /// `materialize` rely on before talking to Vault.
    pub fn auth(&self) -> Result<VaultAuth<'_>> {
        // First require exactly one method *section*, then check that section's
        // credentials are non-blank — a present-but-empty field (e.g.
        // `token: ""`) is a misconfiguration, not a configured method.
        let present = [
            self.token.is_some(),
            self.userpass.is_some(),
            self.approle.is_some(),
        ];
        match present.iter().filter(|&&p| p).count() {
            1 => {}
            0 => bail!(
                "vault config has no auth method; set exactly one of \
                 `token`, `userpass`, or `approle`"
            ),
            n => bail!(
                "vault config sets {n} auth methods; set exactly one of \
                 `token`, `userpass`, or `approle`"
            ),
        }

        if let Some(token) = &self.token {
            if token.is_empty() {
                bail!("vault `token` is empty");
            }
            return Ok(VaultAuth::Token(token));
        }
        if let Some(userpass) = &self.userpass {
            if userpass.username.is_empty() || userpass.password.is_empty() {
                bail!("vault `userpass` needs a non-empty `username` and `password`");
            }
            return Ok(VaultAuth::UserPass(userpass));
        }
        if let Some(approle) = &self.approle {
            if approle.role_id.is_empty() || approle.secret_id.is_empty() {
                bail!("vault `approle` needs a non-empty `role_id` and `secret_id`");
            }
            return Ok(VaultAuth::AppRole(approle));
        }
        // Unreachable: the count check above guarantees one section is present.
        unreachable!("exactly one auth section is present")
    }
}

/// Load and parse the secrets file, warning (Unix) if its permissions are
/// looser than `0600`.
pub fn load(path: &Path) -> Result<Secrets> {
    warn_if_too_open(path);
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading secrets {}", path.display()))?;
    serde_yml::from_str(&text).with_context(|| format!("parsing secrets {}", path.display()))
}

#[cfg(unix)]
fn warn_if_too_open(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            eprintln!(
                "warning: secrets file {} is group/world-accessible (mode {mode:o}); chmod 600 it",
                path.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_too_open(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_s3_secrets() {
        let s: Secrets =
            serde_yml::from_str("s3:\n  access_key_id: id\n  secret_access_key: key\n").unwrap();
        let s3 = s.s3.expect("s3 secrets present");
        assert_eq!(s3.access_key_id, "id");
        assert_eq!(s3.secret_access_key, "key");
    }

    #[test]
    fn s3_section_is_optional() {
        let s: Secrets = serde_yml::from_str("{}\n").unwrap();
        assert!(s.s3.is_none());
    }

    /// Parse a `vault:` section from YAML, panicking on error.
    fn vault_from(yaml: &str) -> VaultSecrets {
        serde_yml::from_str::<Secrets>(yaml)
            .expect("parse secrets")
            .vault
            .expect("vault section present")
    }

    #[test]
    fn token_auth_resolves() {
        let v = vault_from("vault:\n  addr: https://v\n  token: s.tok\n");
        match v.auth().unwrap() {
            VaultAuth::Token(t) => assert_eq!(t, "s.tok"),
            other => panic!("expected token auth, got {other:?}"),
        }
    }

    #[test]
    fn userpass_auth_resolves_with_default_mount() {
        let v = vault_from(
            "vault:\n  addr: https://v\n  userpass:\n    username: u\n    password: p\n",
        );
        match v.auth().unwrap() {
            VaultAuth::UserPass(up) => {
                assert_eq!(up.username, "u");
                assert_eq!(up.password, "p");
                assert_eq!(up.mount, "userpass", "mount should default");
            }
            other => panic!("expected userpass auth, got {other:?}"),
        }
    }

    #[test]
    fn approle_auth_resolves_with_explicit_mount() {
        let v = vault_from(
            "vault:\n  addr: https://v\n  approle:\n    role_id: r\n    secret_id: s\n    mount: ci-approle\n",
        );
        match v.auth().unwrap() {
            VaultAuth::AppRole(ar) => {
                assert_eq!(ar.role_id, "r");
                assert_eq!(ar.secret_id, "s");
                assert_eq!(ar.mount, "ci-approle");
            }
            other => panic!("expected approle auth, got {other:?}"),
        }
    }

    #[test]
    fn no_auth_method_is_an_error() {
        let v = vault_from("vault:\n  addr: https://v\n");
        assert!(v.auth().is_err(), "zero auth methods must error");
    }

    #[test]
    fn two_auth_methods_are_ambiguous() {
        let v = vault_from(
            "vault:\n  addr: https://v\n  token: s.tok\n  userpass:\n    username: u\n    password: p\n",
        );
        assert!(v.auth().is_err(), "more than one auth method must error");
    }

    #[test]
    fn an_empty_token_is_rejected() {
        // A present-but-blank credential (e.g. `token: ""`) is a misconfig, not
        // a valid method — reject it here rather than at a vaguer Vault error.
        let v = vault_from("vault:\n  addr: https://v\n  token: \"\"\n");
        assert!(v.auth().is_err(), "an empty token must error");
    }

    #[test]
    fn a_blank_userpass_field_is_rejected() {
        let v = vault_from(
            "vault:\n  addr: https://v\n  userpass:\n    username: u\n    password: \"\"\n",
        );
        assert!(v.auth().is_err(), "a blank password must error");
    }

    #[test]
    fn a_key_entry_uses_custom_field_names() {
        let v = vault_from(
            "vault:\n  addr: https://v\n  token: t\n  keys:\n    \
             release-signing:\n      path: ces-kv/gpg/pvt\n      \
             private_key_field: key\n      passphrase_field: pass\n",
        );
        let entry = &v.keys["release-signing"];
        assert_eq!(entry.path, "ces-kv/gpg/pvt");
        assert_eq!(entry.private_key_field, "key");
        assert_eq!(entry.passphrase_field, "pass");
    }

    #[test]
    fn a_key_entry_defaults_the_field_names() {
        let v = vault_from(
            "vault:\n  addr: https://v\n  token: t\n  keys:\n    \
             k:\n      path: secret/data/crt/key\n",
        );
        let entry = &v.keys["k"];
        assert_eq!(entry.private_key_field, "private-key");
        assert_eq!(entry.passphrase_field, "passphrase");
    }
}
