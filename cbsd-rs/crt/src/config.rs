// crt — configuration loading.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The non-secret `crt.config.yaml` (design §9): the component name, the store
//! backend (local-fs or S3), the namespace/channel map (release-name
//! resolution + branding), and the configured risk components. Credentials
//! live in `crt.secrets.yaml` (see `crate::secrets`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crt_core::{Branding, ReleaseKey};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub component: String,
    pub store: StoreConfig,
    /// Destination repository for `release materialize` (design §8/§9): where
    /// the linear `release/<name>` branch is built. Holds the configured
    /// identifier (e.g. a `clyso/ceph` slug used for the deferred push); the
    /// `--repo <path>` flag supplies the local working copy and overrides it.
    #[serde(default)]
    pub destination_repo: Option<String>,
    /// Allowed risk-component labels (design §9). An **empty** list (or an
    /// absent key) disables validation — any component is accepted.
    #[serde(default)]
    pub risk_components: Vec<String>,
    /// Namespace → channel map driving release-name resolution (design §9).
    /// `BTreeMap` so iteration — and thus ambiguity detection — is
    /// deterministic.
    #[serde(default)]
    pub namespaces: BTreeMap<String, Namespace>,
    /// Published location of Clyso's OpenPGP public key, used by
    /// `release verify` (design §6.1/§9). A `file:`-less path or an `http(s)`
    /// URL; the `--public-key` flag overrides it.
    #[serde(default)]
    pub public_key_url: Option<String>,
    /// Which `vault.keys` entry to sign with (`seal` / `materialize`). Names a
    /// key in the secrets file; defaults to [`DEFAULT_GPG_KEY_NAME`] when unset.
    #[serde(default)]
    pub gpg_private_key: Option<String>,
}

/// Default `vault.keys` entry name when `gpg_private_key` is unset.
pub const DEFAULT_GPG_KEY_NAME: &str = "gpg_signing_private";

/// A configured namespace: a set of named channels (design §9).
#[derive(Debug, Deserialize)]
pub struct Namespace {
    #[serde(default)]
    pub channels: BTreeMap<String, Channel>,
}

/// A configured channel. The channel **key** (e.g. `ces`) is the resolution
/// prefix; `branding` is snapshotted into the manifest at seal time (§7.2) and
/// surfaced by `release info` here. (The `upstream` block from §9 has no M2
/// consumer, so it is not modelled yet — serde ignores it until M4 needs it.)
#[derive(Debug, Deserialize)]
pub struct Channel {
    pub branding: Branding,
}

/// The store backend. Externally tagged: `store: { local: <path> }` or
/// `store: { s3: { endpoint, region, bucket, prefix } }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreConfig {
    Local(PathBuf),
    S3(S3Config),
}

#[derive(Debug, Deserialize)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
}

impl Config {
    /// Resolve a release name to its `ReleaseKey` by prefix-matching the name
    /// against the configured channels (design §9). A channel key `C` matches
    /// name `N` when `N == C` or `N` starts with `C-` (so `ces` matches
    /// `ces-v18.2.0` but not `cesx-1`). The **most specific** (longest) matching
    /// channel wins, so `ces-lts` is preferred over `ces` for `ces-lts-v1`. A
    /// tie at the longest length across distinct channels is ambiguous and
    /// rejected; no match is rejected.
    pub fn resolve_release_key(&self, name: &str) -> Result<ReleaseKey> {
        let mut best: Option<(&str, &str)> = None;
        let mut ambiguous = false;
        for (namespace, ns) in &self.namespaces {
            for channel in ns.channels.keys() {
                if name != channel && !name.starts_with(&format!("{channel}-")) {
                    continue;
                }
                match best {
                    Some((_, b)) if channel.len() < b.len() => {}
                    Some((_, b)) if channel.len() == b.len() => ambiguous = true,
                    _ => {
                        best = Some((namespace, channel));
                        ambiguous = false;
                    }
                }
            }
        }
        match best {
            None => bail!(
                "no configured channel matches release name {name:?}; \
                 check `namespaces.*.channels` in the config"
            ),
            Some(_) if ambiguous => bail!(
                "release name {name:?} matches more than one channel equally; \
                 channel prefixes are ambiguous"
            ),
            Some((namespace, channel)) => Ok(ReleaseKey {
                namespace: namespace.to_owned(),
                channel: channel.to_owned(),
                name: name.to_owned(),
            }),
        }
    }

    /// Validate a risk-component label against `risk_components` (design §9). An
    /// empty configured list disables validation (any component is accepted).
    pub fn validate_risk_component(&self, component: &str) -> Result<()> {
        if self.risk_components.is_empty() || self.risk_components.iter().any(|c| c == component) {
            return Ok(());
        }
        bail!(
            "risk component {component:?} is not one of the configured \
             risk_components {:?}",
            self.risk_components
        )
    }

    /// The `vault.keys` entry name to sign with — the configured
    /// `gpg_private_key`, or [`DEFAULT_GPG_KEY_NAME`] when unset.
    pub fn gpg_private_key_name(&self) -> &str {
        self.gpg_private_key
            .as_deref()
            .unwrap_or(DEFAULT_GPG_KEY_NAME)
    }
}

/// Load and parse the config file at `path`.
pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    serde_yml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_local_store() {
        let cfg: Config =
            serde_yml::from_str("component: ceph\nstore:\n  local: /tmp/store\n").unwrap();
        assert_eq!(cfg.component, "ceph");
        match cfg.store {
            StoreConfig::Local(p) => assert_eq!(p, PathBuf::from("/tmp/store")),
            StoreConfig::S3(_) => panic!("expected a local store"),
        }
    }

    #[test]
    fn parses_an_s3_store() {
        let yaml = r"
component: ceph
store:
  s3:
    endpoint: https://s3.example.com
    region: us-east-1
    bucket: b
    prefix: crt/
";
        let cfg: Config = serde_yml::from_str(yaml).unwrap();
        match cfg.store {
            StoreConfig::S3(s3) => {
                assert_eq!(s3.bucket, "b");
                assert_eq!(s3.prefix, "crt/");
            }
            StoreConfig::Local(_) => panic!("expected an s3 store"),
        }
    }

    /// A config with two namespaces and the channels named in `channels` (each
    /// gets throwaway branding), for resolution tests.
    fn config_with(namespaces: &[(&str, &[&str])]) -> Config {
        let mut ns_map = BTreeMap::new();
        for (ns, channels) in namespaces {
            let mut chans = BTreeMap::new();
            for channel in *channels {
                chans.insert(
                    (*channel).to_owned(),
                    Channel {
                        branding: Branding {
                            display_name: format!("{channel} display"),
                            blurb: "b".to_owned(),
                            footer: "f".to_owned(),
                        },
                    },
                );
            }
            ns_map.insert((*ns).to_owned(), Namespace { channels: chans });
        }
        Config {
            component: "ceph".to_owned(),
            store: StoreConfig::Local(PathBuf::from("/tmp/store")),
            destination_repo: None,
            risk_components: vec![],
            namespaces: ns_map,
            public_key_url: None,
            gpg_private_key: None,
        }
    }

    #[test]
    fn resolves_a_name_to_its_channel() {
        let cfg = config_with(&[("clyso-enterprise", &["ces"])]);
        let key = cfg.resolve_release_key("ces-v18.2.0").unwrap();
        assert_eq!(key.namespace, "clyso-enterprise");
        assert_eq!(key.channel, "ces");
        assert_eq!(key.name, "ces-v18.2.0");
    }

    #[test]
    fn resolves_an_exact_channel_name() {
        // The bare channel name (no `-suffix`) resolves to that channel.
        let cfg = config_with(&[("clyso-enterprise", &["ces"])]);
        assert_eq!(cfg.resolve_release_key("ces").unwrap().channel, "ces");
    }

    #[test]
    fn prefers_the_longest_matching_channel() {
        // `ces` and `ces-lts` both match `ces-lts-v1`; the longer key wins.
        let cfg = config_with(&[("clyso-enterprise", &["ces", "ces-lts"])]);
        assert_eq!(
            cfg.resolve_release_key("ces-lts-v1").unwrap().channel,
            "ces-lts"
        );
        // …but a plain `ces-v1` only matches `ces`.
        assert_eq!(cfg.resolve_release_key("ces-v1").unwrap().channel, "ces");
    }

    #[test]
    fn rejects_an_unmatched_name() {
        let cfg = config_with(&[("clyso-enterprise", &["ces"])]);
        assert!(cfg.resolve_release_key("nightly-v1").is_err());
        // A near-miss that is not a prefix boundary must not match.
        assert!(cfg.resolve_release_key("cesx-1").is_err());
    }

    #[test]
    fn rejects_an_ambiguous_name() {
        // The same channel key in two namespaces is an unresolvable tie.
        let cfg = config_with(&[("ns-a", &["ces"]), ("ns-b", &["ces"])]);
        assert!(cfg.resolve_release_key("ces-v1").is_err());
    }

    #[test]
    fn risk_component_validation() {
        let mut cfg = config_with(&[("clyso-enterprise", &["ces"])]);
        // Empty list ⇒ anything goes.
        assert!(cfg.validate_risk_component("anything").is_ok());

        cfg.risk_components = vec!["rgw".to_owned(), "dashboard".to_owned()];
        assert!(cfg.validate_risk_component("rgw").is_ok());
        assert!(cfg.validate_risk_component("mds").is_err());
    }

    #[test]
    fn gpg_private_key_name_defaults_then_overrides() {
        let mut cfg = config_with(&[("clyso-enterprise", &["ces"])]);
        assert_eq!(cfg.gpg_private_key_name(), DEFAULT_GPG_KEY_NAME);
        cfg.gpg_private_key = Some("release-signing".to_owned());
        assert_eq!(cfg.gpg_private_key_name(), "release-signing");
    }
}
