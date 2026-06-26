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

//! The cbscore secrets file format (design 004).
//!
//! Source: `cbscore/utils/secrets/models.py`. The [`Secrets`] container holds
//! four maps — git, storage, signing, registry — each keyed by the resource it
//! applies to. Every entry is `plain` or `vault`; a `vault` entry also carries
//! `key` (the Vault path to read).
//!
//! **Discrimination.** Each family is a single enum whose `Deserialize` mirrors
//! Python's per-family discriminator function, dispatching on `(creds, type)`
//! (registry on `creds` alone). The Rust port **adds an explicit
//! `type: ssh | token | https` field to git secrets** — the one
//! operator-visible format change (design 004): it makes the git discriminator
//! a uniform `(creds, type)` match like storage and signing, instead of
//! inspecting field shape. Serialization goes through the same raw form, so a
//! parsed secret re-emits the wire shape it came from.
//!
//! These are the resolution-independent *format* types. `SecretsMgr` (which
//! resolves a plain entry, or reads a `vault` entry's `key` from Vault, and
//! wraps the result in a redacted `Password`/`SecureUrl`) lands with its first
//! consumer in M2/C3–C4a; nothing here renders or returns plaintext to a tool.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::schema::schema_v1;

/// The secrets container: four resource-keyed maps (design 004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Secrets {
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    /// Keyed by repo URL/host (longest-prefix match at resolution).
    #[serde(default)]
    pub git: BTreeMap<String, GitSecret>,
    /// Keyed by S3 URL (exact match at resolution).
    #[serde(default)]
    pub storage: BTreeMap<String, StorageSecret>,
    /// Keyed by signing id (exact match at resolution).
    #[serde(default)]
    pub sign: BTreeMap<String, SigningSecret>,
    /// Keyed by registry URL (longest-prefix match at resolution).
    #[serde(default)]
    pub registry: BTreeMap<String, RegistrySecret>,
}

impl Secrets {
    /// The highest `schema-version` this build understands.
    pub const SCHEMA_MAX: u32 = 1;
    /// Human-facing format name for schema-version errors.
    pub const SCHEMA_FORMAT: &'static str = "secrets";

    /// Merge `other` into `self`, with `other`'s keys overriding per map
    /// (mirrors Python's `Secrets.merge`; used by `Config::get_secrets`).
    pub fn merge(&mut self, other: Secrets) {
        self.git.extend(other.git);
        self.storage.extend(other.storage);
        self.sign.extend(other.sign);
        self.registry.extend(other.registry);
    }
}

/// The `creds` discriminator shared by every family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Creds {
    Plain,
    Vault,
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// A git credential, discriminated by `(creds, type)`. `token` is plain-only
/// (there is no `vault-token`).
#[derive(Clone, PartialEq, Eq)]
pub enum GitSecret {
    PlainSsh {
        ssh_key: String,
        username: String,
    },
    PlainToken {
        token: String,
        username: String,
    },
    PlainHttps {
        username: String,
        password: String,
    },
    VaultSsh {
        key: String,
        ssh_key: String,
        username: String,
    },
    VaultHttps {
        key: String,
        username: String,
        password: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum GitKind {
    Ssh,
    Token,
    Https,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct GitSecretRaw {
    creds: Creds,
    #[serde(rename = "type")]
    kind: GitKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

impl<'de> Deserialize<'de> for GitSecret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = GitSecretRaw::deserialize(d)?;
        match (r.creds, r.kind) {
            (Creds::Plain, GitKind::Ssh) => Ok(Self::PlainSsh {
                ssh_key: req::<D>(r.ssh_key, "git", "ssh-key")?,
                username: req::<D>(r.username, "git", "username")?,
            }),
            (Creds::Plain, GitKind::Token) => Ok(Self::PlainToken {
                token: req::<D>(r.token, "git", "token")?,
                username: req::<D>(r.username, "git", "username")?,
            }),
            (Creds::Plain, GitKind::Https) => Ok(Self::PlainHttps {
                username: req::<D>(r.username, "git", "username")?,
                password: req::<D>(r.password, "git", "password")?,
            }),
            (Creds::Vault, GitKind::Ssh) => Ok(Self::VaultSsh {
                key: req::<D>(r.key, "git", "key")?,
                ssh_key: req::<D>(r.ssh_key, "git", "ssh-key")?,
                username: req::<D>(r.username, "git", "username")?,
            }),
            (Creds::Vault, GitKind::Https) => Ok(Self::VaultHttps {
                key: req::<D>(r.key, "git", "key")?,
                username: req::<D>(r.username, "git", "username")?,
                password: req::<D>(r.password, "git", "password")?,
            }),
            (Creds::Vault, GitKind::Token) => {
                Err(D::Error::custom("git secret: 'token' has no vault variant"))
            }
        }
    }
}

impl Serialize for GitSecret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let raw = match self.clone() {
            Self::PlainSsh { ssh_key, username } => GitSecretRaw {
                creds: Creds::Plain,
                kind: GitKind::Ssh,
                key: None,
                ssh_key: Some(ssh_key),
                token: None,
                username: Some(username),
                password: None,
            },
            Self::PlainToken { token, username } => GitSecretRaw {
                creds: Creds::Plain,
                kind: GitKind::Token,
                key: None,
                ssh_key: None,
                token: Some(token),
                username: Some(username),
                password: None,
            },
            Self::PlainHttps { username, password } => GitSecretRaw {
                creds: Creds::Plain,
                kind: GitKind::Https,
                key: None,
                ssh_key: None,
                token: None,
                username: Some(username),
                password: Some(password),
            },
            Self::VaultSsh {
                key,
                ssh_key,
                username,
            } => GitSecretRaw {
                creds: Creds::Vault,
                kind: GitKind::Ssh,
                key: Some(key),
                ssh_key: Some(ssh_key),
                token: None,
                username: Some(username),
                password: None,
            },
            Self::VaultHttps {
                key,
                username,
                password,
            } => GitSecretRaw {
                creds: Creds::Vault,
                kind: GitKind::Https,
                key: Some(key),
                ssh_key: None,
                token: None,
                username: Some(username),
                password: Some(password),
            },
        };
        raw.serialize(s)
    }
}

// ---------------------------------------------------------------------------
// storage
// ---------------------------------------------------------------------------

/// An S3 storage credential. `type` is always `s3`.
#[derive(Clone, PartialEq, Eq)]
pub enum StorageSecret {
    PlainS3 {
        access_id: String,
        secret_id: String,
    },
    VaultS3 {
        key: String,
        access_id: String,
        secret_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StorageKind {
    S3,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct StorageSecretRaw {
    creds: Creds,
    #[serde(rename = "type")]
    kind: StorageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_id: Option<String>,
}

impl<'de> Deserialize<'de> for StorageSecret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = StorageSecretRaw::deserialize(d)?;
        let StorageKind::S3 = r.kind;
        match r.creds {
            Creds::Plain => Ok(Self::PlainS3 {
                access_id: req::<D>(r.access_id, "storage", "access-id")?,
                secret_id: req::<D>(r.secret_id, "storage", "secret-id")?,
            }),
            Creds::Vault => Ok(Self::VaultS3 {
                key: req::<D>(r.key, "storage", "key")?,
                access_id: req::<D>(r.access_id, "storage", "access-id")?,
                secret_id: req::<D>(r.secret_id, "storage", "secret-id")?,
            }),
        }
    }
}

impl Serialize for StorageSecret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let raw = match self.clone() {
            Self::PlainS3 {
                access_id,
                secret_id,
            } => StorageSecretRaw {
                creds: Creds::Plain,
                kind: StorageKind::S3,
                key: None,
                access_id: Some(access_id),
                secret_id: Some(secret_id),
            },
            Self::VaultS3 {
                key,
                access_id,
                secret_id,
            } => StorageSecretRaw {
                creds: Creds::Vault,
                kind: StorageKind::S3,
                key: Some(key),
                access_id: Some(access_id),
                secret_id: Some(secret_id),
            },
        };
        raw.serialize(s)
    }
}

// ---------------------------------------------------------------------------
// signing
// ---------------------------------------------------------------------------

/// A signing credential: a plain armored GPG key, several Vault-backed GPG key
/// shapes, or a Vault transit key.
#[derive(Clone, PartialEq, Eq)]
pub enum SigningSecret {
    PlainGpgKey {
        private_key: String,
        public_key: Option<String>,
        passphrase: Option<String>,
        email: String,
    },
    VaultGpgSingleKey {
        key: String,
        private_key: String,
        public_key: Option<String>,
        passphrase: Option<String>,
        email: String,
    },
    VaultGpgPvtKey {
        key: String,
        private_key: String,
        passphrase: Option<String>,
        email: String,
    },
    VaultGpgPubKey {
        key: String,
        public_key: String,
        email: String,
    },
    VaultTransit {
        key: String,
        mount: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SigningKind {
    GpgArmorKey,
    GpgSingleKey,
    GpgPvtKey,
    GpgPubKey,
    Transit,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SigningSecretRaw {
    creds: Creds,
    #[serde(rename = "type")]
    kind: SigningKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passphrase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mount: Option<String>,
}

impl<'de> Deserialize<'de> for SigningSecret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = SigningSecretRaw::deserialize(d)?;
        match (r.creds, r.kind) {
            (Creds::Plain, SigningKind::GpgArmorKey) => Ok(Self::PlainGpgKey {
                private_key: req::<D>(r.private_key, "signing", "private-key")?,
                public_key: r.public_key,
                passphrase: r.passphrase,
                email: req::<D>(r.email, "signing", "email")?,
            }),
            (Creds::Vault, SigningKind::GpgSingleKey) => Ok(Self::VaultGpgSingleKey {
                key: req::<D>(r.key, "signing", "key")?,
                private_key: req::<D>(r.private_key, "signing", "private-key")?,
                public_key: r.public_key,
                passphrase: r.passphrase,
                email: req::<D>(r.email, "signing", "email")?,
            }),
            (Creds::Vault, SigningKind::GpgPvtKey) => Ok(Self::VaultGpgPvtKey {
                key: req::<D>(r.key, "signing", "key")?,
                private_key: req::<D>(r.private_key, "signing", "private-key")?,
                passphrase: r.passphrase,
                email: req::<D>(r.email, "signing", "email")?,
            }),
            (Creds::Vault, SigningKind::GpgPubKey) => Ok(Self::VaultGpgPubKey {
                key: req::<D>(r.key, "signing", "key")?,
                public_key: req::<D>(r.public_key, "signing", "public-key")?,
                email: req::<D>(r.email, "signing", "email")?,
            }),
            (Creds::Vault, SigningKind::Transit) => Ok(Self::VaultTransit {
                key: req::<D>(r.key, "signing", "key")?,
                mount: req::<D>(r.mount, "signing", "mount")?,
            }),
            (creds, kind) => Err(D::Error::custom(format!(
                "signing secret: invalid (creds={creds:?}, type={kind:?}) combination"
            ))),
        }
    }
}

impl Serialize for SigningSecret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let base = || SigningSecretRaw {
            creds: Creds::Vault,
            kind: SigningKind::Transit,
            key: None,
            private_key: None,
            public_key: None,
            passphrase: None,
            email: None,
            mount: None,
        };
        let raw = match self.clone() {
            Self::PlainGpgKey {
                private_key,
                public_key,
                passphrase,
                email,
            } => SigningSecretRaw {
                creds: Creds::Plain,
                kind: SigningKind::GpgArmorKey,
                private_key: Some(private_key),
                public_key,
                passphrase,
                email: Some(email),
                ..base()
            },
            Self::VaultGpgSingleKey {
                key,
                private_key,
                public_key,
                passphrase,
                email,
            } => SigningSecretRaw {
                creds: Creds::Vault,
                kind: SigningKind::GpgSingleKey,
                key: Some(key),
                private_key: Some(private_key),
                public_key,
                passphrase,
                email: Some(email),
                ..base()
            },
            Self::VaultGpgPvtKey {
                key,
                private_key,
                passphrase,
                email,
            } => SigningSecretRaw {
                creds: Creds::Vault,
                kind: SigningKind::GpgPvtKey,
                key: Some(key),
                private_key: Some(private_key),
                passphrase,
                email: Some(email),
                ..base()
            },
            Self::VaultGpgPubKey {
                key,
                public_key,
                email,
            } => SigningSecretRaw {
                creds: Creds::Vault,
                kind: SigningKind::GpgPubKey,
                key: Some(key),
                public_key: Some(public_key),
                email: Some(email),
                ..base()
            },
            Self::VaultTransit { key, mount } => SigningSecretRaw {
                creds: Creds::Vault,
                kind: SigningKind::Transit,
                key: Some(key),
                mount: Some(mount),
                ..base()
            },
        };
        raw.serialize(s)
    }
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

/// A container-registry credential, discriminated by `creds` alone.
#[derive(Clone, PartialEq, Eq)]
pub enum RegistrySecret {
    Plain {
        username: String,
        password: String,
        address: String,
    },
    Vault {
        key: String,
        username: String,
        password: String,
        address: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RegistrySecretRaw {
    creds: Creds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    address: Option<String>,
}

impl<'de> Deserialize<'de> for RegistrySecret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = RegistrySecretRaw::deserialize(d)?;
        match r.creds {
            Creds::Plain => Ok(Self::Plain {
                username: req::<D>(r.username, "registry", "username")?,
                password: req::<D>(r.password, "registry", "password")?,
                address: req::<D>(r.address, "registry", "address")?,
            }),
            Creds::Vault => Ok(Self::Vault {
                key: req::<D>(r.key, "registry", "key")?,
                username: req::<D>(r.username, "registry", "username")?,
                password: req::<D>(r.password, "registry", "password")?,
                address: req::<D>(r.address, "registry", "address")?,
            }),
        }
    }
}

impl Serialize for RegistrySecret {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let raw = match self.clone() {
            Self::Plain {
                username,
                password,
                address,
            } => RegistrySecretRaw {
                creds: Creds::Plain,
                key: None,
                username: Some(username),
                password: Some(password),
                address: Some(address),
            },
            Self::Vault {
                key,
                username,
                password,
                address,
            } => RegistrySecretRaw {
                creds: Creds::Vault,
                key: Some(key),
                username: Some(username),
                password: Some(password),
                address: Some(address),
            },
        };
        raw.serialize(s)
    }
}

/// Pull a required field out of a parsed raw secret, erroring with a clear
/// "family: missing 'field'" message when it is absent for the chosen variant.
fn req<'de, D: Deserializer<'de>>(
    value: Option<String>,
    family: &str,
    field: &str,
) -> Result<String, D::Error> {
    value.ok_or_else(|| D::Error::custom(format!("{family} secret: missing '{field}'")))
}

// Hand-written `Debug` for every credential family: it prints only the variant
// name, never the plaintext fields (`ssh-key`, `password`, `token`,
// `private-key`, `passphrase`). The derived `Debug` would expose those via
// `{:?}` — an invariant-4 leak the moment a secret reaches an error context or
// log (e.g. `SecretsMgr` formatting a failed resolution). `Secrets` keeps its
// derived `Debug`, which is safe because it formats these through these impls.

impl fmt::Debug for GitSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            Self::PlainSsh { .. } => "PlainSsh",
            Self::PlainToken { .. } => "PlainToken",
            Self::PlainHttps { .. } => "PlainHttps",
            Self::VaultSsh { .. } => "VaultSsh",
            Self::VaultHttps { .. } => "VaultHttps",
        };
        write!(f, "GitSecret::{variant}(<redacted>)")
    }
}

impl fmt::Debug for StorageSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            Self::PlainS3 { .. } => "PlainS3",
            Self::VaultS3 { .. } => "VaultS3",
        };
        write!(f, "StorageSecret::{variant}(<redacted>)")
    }
}

impl fmt::Debug for SigningSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            Self::PlainGpgKey { .. } => "PlainGpgKey",
            Self::VaultGpgSingleKey { .. } => "VaultGpgSingleKey",
            Self::VaultGpgPvtKey { .. } => "VaultGpgPvtKey",
            Self::VaultGpgPubKey { .. } => "VaultGpgPubKey",
            Self::VaultTransit { .. } => "VaultTransit",
        };
        write!(f, "SigningSecret::{variant}(<redacted>)")
    }
}

impl fmt::Debug for RegistrySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let variant = match self {
            Self::Plain { .. } => "Plain",
            Self::Vault { .. } => "Vault",
        };
        write!(f, "RegistrySecret::{variant}(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a single secret value through JSON (the serde data model is
    /// shared with YAML; the YAML-specific golden tests live in `cbscore`).
    fn rt<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn git_variants_discriminate_on_creds_and_type() {
        let plain_ssh = GitSecret::PlainSsh {
            ssh_key: "KEY".to_string(),
            username: "git".to_string(),
        };
        assert_eq!(plain_ssh, rt(&plain_ssh));
        let vault_https = GitSecret::VaultHttps {
            key: "kv/data/git".to_string(),
            username: "u".to_string(),
            password: "p".to_string(),
        };
        assert_eq!(vault_https, rt(&vault_https));

        // The added `type` tag is emitted and required.
        let json = serde_json::to_value(&plain_ssh).unwrap();
        assert_eq!(json["creds"], "plain");
        assert_eq!(json["type"], "ssh");
        assert_eq!(json["ssh-key"], "KEY");
    }

    #[test]
    fn git_token_has_no_vault_variant() {
        let json = r#"{"creds":"vault","type":"token","key":"k","token":"t","username":"u"}"#;
        let err = serde_json::from_str::<GitSecret>(json).unwrap_err();
        assert!(err.to_string().contains("no vault variant"), "{err}");
    }

    #[test]
    fn git_type_is_required() {
        // A shape-only (type-less) git entry is rejected (the wire change).
        let json = r#"{"creds":"plain","ssh-key":"k","username":"u"}"#;
        assert!(serde_json::from_str::<GitSecret>(json).is_err());
    }

    #[test]
    fn git_missing_required_field_is_a_clear_error() {
        let json = r#"{"creds":"plain","type":"https","username":"u"}"#;
        let err = serde_json::from_str::<GitSecret>(json).unwrap_err();
        assert!(err.to_string().contains("missing 'password'"), "{err}");
    }

    #[test]
    fn storage_signing_registry_round_trip() {
        let s = StorageSecret::VaultS3 {
            key: "kv/data/s3".to_string(),
            access_id: "AK".to_string(),
            secret_id: "SK".to_string(),
        };
        assert_eq!(s, rt(&s));

        let sign = SigningSecret::PlainGpgKey {
            private_key: "PRIV".to_string(),
            public_key: None,
            passphrase: Some("pp".to_string()),
            email: "ceph@clyso.com".to_string(),
        };
        assert_eq!(sign, rt(&sign));

        let transit = SigningSecret::VaultTransit {
            key: "kv/data/transit".to_string(),
            mount: "transit".to_string(),
        };
        assert_eq!(transit, rt(&transit));

        let reg = RegistrySecret::Plain {
            username: "u".to_string(),
            password: "p".to_string(),
            address: "harbor.clyso.com".to_string(),
        };
        assert_eq!(reg, rt(&reg));
    }

    #[test]
    fn signing_invalid_combination_errors() {
        // plain + transit is not a valid variant.
        let json = r#"{"creds":"plain","type":"transit","mount":"m"}"#;
        let err = serde_json::from_str::<SigningSecret>(json).unwrap_err();
        assert!(err.to_string().contains("invalid"), "{err}");
    }

    #[test]
    fn secrets_merge_overrides_per_map() {
        let mut a = Secrets {
            schema_version: 1,
            git: BTreeMap::from([(
                "github.com".to_string(),
                GitSecret::PlainHttps {
                    username: "old".to_string(),
                    password: "old".to_string(),
                },
            )]),
            storage: BTreeMap::new(),
            sign: BTreeMap::new(),
            registry: BTreeMap::new(),
        };
        let b = Secrets {
            schema_version: 1,
            git: BTreeMap::from([(
                "github.com".to_string(),
                GitSecret::PlainHttps {
                    username: "new".to_string(),
                    password: "new".to_string(),
                },
            )]),
            storage: BTreeMap::new(),
            sign: BTreeMap::new(),
            registry: BTreeMap::new(),
        };
        a.merge(b);
        assert_eq!(
            a.git["github.com"],
            GitSecret::PlainHttps {
                username: "new".to_string(),
                password: "new".to_string(),
            }
        );
    }

    #[test]
    fn empty_secrets_default_marker_and_maps() {
        let s: Secrets = serde_json::from_str("{}").unwrap();
        assert_eq!(s.schema_version, 1);
        assert!(s.git.is_empty() && s.storage.is_empty());
    }

    #[test]
    fn debug_never_leaks_plaintext() {
        // Directly and through the container's derived Debug, no plaintext
        // credential field appears (invariant 4).
        let git = GitSecret::PlainSsh {
            ssh_key: "TOP-SECRET-KEY".to_string(),
            username: "git".to_string(),
        };
        let dbg = format!("{git:?}");
        assert!(!dbg.contains("TOP-SECRET-KEY"), "leaked: {dbg}");
        assert!(
            dbg.contains("PlainSsh") && dbg.contains("redacted"),
            "{dbg}"
        );

        let mut s = Secrets {
            schema_version: 1,
            git: BTreeMap::new(),
            storage: BTreeMap::new(),
            sign: BTreeMap::new(),
            registry: BTreeMap::new(),
        };
        s.sign.insert(
            "ceph".to_string(),
            SigningSecret::PlainGpgKey {
                private_key: "PRIVATE-MATERIAL".to_string(),
                public_key: None,
                passphrase: Some("PASSPHRASE".to_string()),
                email: "e".to_string(),
            },
        );
        let sdbg = format!("{s:?}");
        assert!(
            !sdbg.contains("PRIVATE-MATERIAL") && !sdbg.contains("PASSPHRASE"),
            "leaked through container Debug: {sdbg}"
        );
    }
}
