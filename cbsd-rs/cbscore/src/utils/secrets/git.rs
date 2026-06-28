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

//! Resolving a git URL against a configured git secret (design 004). Source:
//! `cbscore/utils/secrets/git.py`.
//!
//! `plain-https`/`plain-token` fold the credentials into a redacted
//! [`SecureUrl`]; `plain-ssh` materialises a temporary `~/.ssh` key + alias whose
//! [`Drop`] removes the key; no match yields the URL unchanged. Vault-backed git
//! secrets (`vault-ssh`/`vault-https`) read the credential fields from their
//! `ces-kv` `key` through the [`Vault`] client (C4a), then reuse the same plain
//! materialisation.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::{Arc, LazyLock};

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;

use crate::types::GitSecret;
use crate::utils::redact::{CmdArg, Password, SecureUrl};
use crate::utils::secrets::utils::find_best_secret_candidate;
use crate::utils::secrets::{SecretsError, read_vault_secret, vault_field};
use crate::utils::subprocess::{RunOpts, run_cmd};
use crate::utils::vault::Vault;

/// A git URL validator/extractor: `file://`, `git://`, or `http(s)`/`ssh`,
/// optionally credentialed and ported (`git.py:42-66`). Only the `http_*`
/// groups are read here — the credentialed cases all rewrite to `https://`.
static GIT_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        ^
        (?:
            file:///(?P<file_path>(?:[\w\-/]+/)*[\w\-]+(?:\.git)?)
          |
            git://(?P<git_host>[\w.\-]+)(?::(?P<git_port>\d+))?
                /(?P<git_path>(?:[\w\-/]+/)*[\w\-]+(?:\.git)?)
          |
            (?P<http_protocol>https?|ssh)://
                (?:(?P<user>[\w\-.]+)(?::(?P<password>[^@]+))?@)?
                (?P<http_host>[\w.\-]+)(?::(?P<http_port>\d+))?
                /(?P<http_path>(?:[\w\-/]+/)*[\w\-]+(?:\.git)?)
        )
        ",
    )
    .expect("git url regex is valid")
});

/// The `http_host` / `http_port` / `http_path` of an `http(s)`/`ssh` git URL.
struct HttpUrlParts {
    host: String,
    port: Option<String>,
    path: String,
}

impl HttpUrlParts {
    /// `host[:port]`, as `git.py` assembles for the credentialed URL.
    fn host_with_port(&self) -> String {
        match &self.port {
            Some(port) => format!("{}:{}", self.host, port),
            None => self.host.clone(),
        }
    }
}

/// Whether `url` is a syntactically valid git URL (`git.py:238-242`).
fn git_url_is_valid(url: &str) -> bool {
    GIT_URL_RE.is_match(url)
}

/// Extract the host/port/path of an `http(s)`/`ssh` git URL. `None` for a
/// `file://` or `git://` URL (those carry no `http_*` groups) or a non-URL.
fn parse_http_git_url(url: &str) -> Option<HttpUrlParts> {
    let caps = GIT_URL_RE.captures(url)?;
    Some(HttpUrlParts {
        host: caps.name("http_host")?.as_str().to_string(),
        port: caps.name("http_port").map(|m| m.as_str().to_string()),
        path: caps.name("http_path")?.as_str().to_string(),
    })
}

/// A resolved git URL, ready to hand to `git_clone` as its `repo` argument. For
/// an SSH secret it also owns the temporary key's [`Drop`] guard — keep the
/// [`GitUrl`] alive until the clone completes. `Debug` shows the redacted
/// argument (a credentialed URL is censored), never plaintext.
#[derive(Debug)]
pub struct GitUrl {
    arg: CmdArg,
    _ssh: Option<SshKeyGuard>,
}

impl GitUrl {
    /// The clone argument (a redacted [`SecureUrl`], an SSH alias, or the
    /// unchanged URL).
    pub fn arg(&self) -> &CmdArg {
        &self.arg
    }
}

/// Removes the temporary SSH private key when the resolved URL goes out of
/// scope (`git.py:161`). The `~/.ssh/config` and `known_hosts` entries persist,
/// exactly as in Python.
#[derive(Debug)]
struct SshKeyGuard {
    key_path: Utf8PathBuf,
}

impl Drop for SshKeyGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.key_path);
    }
}

/// Resolve `url` against the configured git secrets, choosing the secret whose
/// key is the closest prefix of `url` (`git.py:233-263`). A `vault-*` secret
/// reads its credential fields from `vault` (its `key` in `ces-kv`); a matched
/// `vault-*` secret with no `vault` configured errors.
pub async fn git_url_for(
    url: &str,
    secrets: &BTreeMap<String, GitSecret>,
    ssh_dir: &Utf8Path,
    vault: Option<&Vault>,
) -> Result<GitUrl, SecretsError> {
    if !git_url_is_valid(url) {
        return Err(SecretsError::InvalidUrl(url.to_string()));
    }

    let Some(key) = find_best_secret_candidate(secrets.keys().map(String::as_str), url) else {
        // No configured secret: clone the URL unchanged (public/plain repos —
        // the common case).
        return Ok(GitUrl {
            arg: CmdArg::Plain(url.to_string()),
            _ssh: None,
        });
    };

    match &secrets[key] {
        GitSecret::PlainSsh { ssh_key, username } => {
            ssh_git_url(url, ssh_dir, username, ssh_key).await
        }
        GitSecret::PlainHttps { username, password } => https_git_url(url, username, password),
        GitSecret::PlainToken { token, username } => Ok(GitUrl {
            arg: CmdArg::Secure(Arc::new(build_token_url(url, username, token)?)),
            _ssh: None,
        }),
        // Vault variants: `ssh_key`/`username`/`password` are field *names* in
        // the `ces-kv` secret at `key` (`git.py:120-201`); read them, then reuse
        // the plain materialisation.
        GitSecret::VaultSsh {
            key,
            ssh_key,
            username,
        } => {
            let secret = read_vault_secret(vault, key).await?;
            let ssh_key = vault_field(&secret, ssh_key)?;
            let username = vault_field(&secret, username)?;
            ssh_git_url(url, ssh_dir, &username, &ssh_key).await
        }
        GitSecret::VaultHttps {
            key,
            username,
            password,
        } => {
            let secret = read_vault_secret(vault, key).await?;
            let username = vault_field(&secret, username)?;
            let password = vault_field(&secret, password)?;
            https_git_url(url, &username, &password)
        }
    }
}

/// Materialise an SSH key/alias for `username`+`ssh_key` and wrap it as a
/// [`GitUrl`] (shared by `plain-ssh` and `vault-ssh`).
async fn ssh_git_url(
    url: &str,
    ssh_dir: &Utf8Path,
    username: &str,
    ssh_key: &str,
) -> Result<GitUrl, SecretsError> {
    let (alias, guard) = materialize_ssh(url, ssh_dir, username, ssh_key).await?;
    Ok(GitUrl {
        arg: CmdArg::Plain(alias),
        _ssh: Some(guard),
    })
}

/// Fold `username`+`password` into a redacted HTTPS [`GitUrl`] (shared by
/// `plain-https` and `vault-https`).
fn https_git_url(url: &str, username: &str, password: &str) -> Result<GitUrl, SecretsError> {
    Ok(GitUrl {
        arg: CmdArg::Secure(Arc::new(build_https_url(url, username, password)?)),
        _ssh: None,
    })
}

/// Build `https://{user}:{password}@{host[:port]}/{path}` as a redacted
/// [`SecureUrl`] (`git.py:164-209`, plain branch).
fn build_https_url(url: &str, username: &str, password: &str) -> Result<SecureUrl, SecretsError> {
    let parts = parse_http_git_url(url).ok_or_else(|| SecretsError::InvalidUrl(url.to_string()))?;
    Ok(
        SecureUrl::new("https://{username}:{password}@{host_with_port}/{path}")
            .arg("username", username)
            .secret("password", Password::new(password))
            .arg("host_with_port", parts.host_with_port())
            .arg("path", parts.path),
    )
}

/// Build `https://{user}:{token}@{host[:port]}/{path}` as a redacted
/// [`SecureUrl`] (`git.py:212-230`).
fn build_token_url(url: &str, username: &str, token: &str) -> Result<SecureUrl, SecretsError> {
    let parts = parse_http_git_url(url).ok_or_else(|| SecretsError::InvalidUrl(url.to_string()))?;
    Ok(
        SecureUrl::new("https://{username}:{token}@{host_with_port}/{path}")
            .arg("username", username)
            .secret("token", Password::new(token))
            .arg("host_with_port", parts.host_with_port())
            .arg("path", parts.path),
    )
}

/// Scan the host key, then write the key/config/known_hosts material and return
/// the `<alias>:<repo>` clone target plus the key's cleanup guard
/// (`git.py:70-162`, plain SSH branch).
async fn materialize_ssh(
    url: &str,
    ssh_dir: &Utf8Path,
    username: &str,
    ssh_key: &str,
) -> Result<(String, SshKeyGuard), SecretsError> {
    let parts = parse_http_git_url(url).ok_or_else(|| SecretsError::InvalidUrl(url.to_string()))?;
    let port = parts.port.clone().unwrap_or_else(|| "22".to_string());
    let host_key = ssh_keyscan(&parts.host).await?;
    write_ssh_material(
        ssh_dir,
        &parts.host,
        &port,
        username,
        ssh_key,
        &host_key,
        &parts.path,
    )
}

/// `ssh-keyscan -t rsa <host>` → the host key line (`git.py:96-115`).
async fn ssh_keyscan(host: &str) -> Result<String, SecretsError> {
    let out = run_cmd(
        &[
            CmdArg::from("ssh-keyscan"),
            CmdArg::from("-t"),
            CmdArg::from("rsa"),
            CmdArg::from(host),
        ],
        RunOpts::default(),
    )
    .await
    .map_err(|source| SecretsError::Keyscan {
        host: host.to_string(),
        source,
    })?;
    if out.code != 0 || out.stdout.is_empty() {
        return Err(SecretsError::KeyscanFailed {
            host: host.to_string(),
            stderr: out.stderr,
        });
    }
    Ok(out.stdout)
}

/// Write the key (0600), append the host key to `known_hosts`, and append a
/// `Host` alias to `config` under `ssh_dir` (created 0700 if missing). Returns
/// the `<alias>:<repo>` clone target and the key's cleanup guard. Synchronous
/// (small local writes; `git.py` is sync here too) and offline, so it is unit-
/// testable without `ssh-keyscan`.
fn write_ssh_material(
    ssh_dir: &Utf8Path,
    host: &str,
    port: &str,
    username: &str,
    ssh_key: &str,
    host_key: &str,
    repo: &str,
) -> Result<(String, SshKeyGuard), SecretsError> {
    use std::fs::OpenOptions;
    use std::io::Write as _;

    ensure_ssh_dir(ssh_dir)?;
    let remote_name = random_alias();

    let known_hosts = ssh_dir.join("known_hosts");
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&known_hosts)
        .and_then(|mut f| f.write_all(host_key.as_bytes()))
        .map_err(|source| io_err(&known_hosts, source))?;

    let key_path = ssh_dir.join(format!("id_{remote_name}"));
    std::fs::File::create(&key_path)
        .and_then(|mut f| {
            f.write_all(ssh_key.as_bytes())?;
            f.write_all(b"\n")
        })
        .map_err(|source| io_err(&key_path, source))?;
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .map_err(|source| io_err(&key_path, source))?;

    let config_path = ssh_dir.join("config");
    let block = format!(
        "\nHost {remote_name}\nHostname {host}\nUser {username}\nPort {port}\nIdentityFile {key_path}\n\n"
    );
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config_path)
        .and_then(|mut f| f.write_all(block.as_bytes()))
        .map_err(|source| io_err(&config_path, source))?;

    Ok((format!("{remote_name}:{repo}"), SshKeyGuard { key_path }))
}

/// Create `ssh_dir` with mode 0700 if it does not exist (`git.py:81-82`; the
/// mode is set only when we create it, matching `mkdir(exist_ok=True)`).
fn ensure_ssh_dir(ssh_dir: &Utf8Path) -> Result<(), SecretsError> {
    if ssh_dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(ssh_dir).map_err(|source| io_err(ssh_dir, source))?;
    std::fs::set_permissions(ssh_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|source| io_err(ssh_dir, source))
}

fn io_err(path: &Utf8Path, source: std::io::Error) -> SecretsError {
    SecretsError::Io {
        context: path.to_string(),
        source,
    }
}

/// Ten random ASCII letters — the SSH alias / key-file name
/// (`random.choices(string.ascii_letters, k=10)`, `git.py:84`).
fn random_alias() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    (0..10)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, GitSecret)]) -> BTreeMap<String, GitSecret> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn git_url_validity_matches_python_cases() {
        // Ported from cbscore/utils/secrets/git.py:275-294.
        for (url, valid) in [
            ("file:///home/user/repo.git", true),
            ("file:///home/user/repo", true),
            ("git://github.com/user/repo.git", true),
            ("git://github.com/user/repo", true),
            ("https://github.com/user/repo.git", true),
            ("https://github.com/user/repo", true),
            ("http://user:pass@github.com/user/repo.git", true),
            ("http://user@github.com/user/repo", true),
            ("ssh://user@host.xz:22/path/to/repo.git", true),
            ("ssh://host.xz/path/to/repo", true),
            ("ssh://user:pass@host.xz:22/path/to/repo.git", true),
            ("file://home/user/repo.git", false), // missing third slash
            ("git:/github.com/user/repo.git", false), // missing one slash
            ("https:/github.com/user/repo", false), // missing one slash
            ("ftp://github.com/user/repo.git", false), // unsupported protocol
            ("git://github.com/.git", false),     // missing repo name before .git
        ] {
            assert_eq!(git_url_is_valid(url), valid, "validity of '{url}'");
        }
    }

    #[test]
    fn parse_http_git_url_extracts_host_port_path() {
        let p = parse_http_git_url("https://github.com/ceph/ceph").unwrap();
        assert_eq!(p.host, "github.com");
        assert_eq!(p.port, None);
        assert_eq!(p.path, "ceph/ceph");
        assert_eq!(p.host_with_port(), "github.com");

        let p = parse_http_git_url("ssh://git@gitlab.example.com:2222/group/proj.git").unwrap();
        assert_eq!(p.host, "gitlab.example.com");
        assert_eq!(p.port.as_deref(), Some("2222"));
        assert_eq!(p.path, "group/proj.git");
        assert_eq!(p.host_with_port(), "gitlab.example.com:2222");

        // file:// has no http_* groups.
        assert!(parse_http_git_url("file:///srv/repo.git").is_none());
    }

    #[tokio::test]
    async fn no_matching_secret_yields_the_url_unchanged() {
        let secrets = BTreeMap::new();
        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();
        let url = "https://github.com/ceph/ceph";

        let resolved = git_url_for(url, &secrets, ssh_dir, None).await.unwrap();
        assert_eq!(resolved.arg().plaintext(), url);
        assert!(resolved._ssh.is_none());
    }

    #[tokio::test]
    async fn invalid_url_is_rejected() {
        let secrets = BTreeMap::new();
        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();
        let err = git_url_for("ftp://nope/x", &secrets, ssh_dir, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::InvalidUrl(_)), "{err}");
    }

    #[tokio::test]
    async fn https_and_token_secrets_fold_and_redact_credentials() {
        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();
        let url = "https://github.com/ceph/ceph";

        let https = map(&[(
            "github.com",
            GitSecret::PlainHttps {
                username: "git".to_string(),
                password: "s3cr3t".to_string(),
            },
        )]);
        let resolved = git_url_for(url, &https, ssh_dir, None).await.unwrap();
        assert_eq!(
            resolved.arg().plaintext(),
            "https://git:s3cr3t@github.com/ceph/ceph"
        );
        assert!(!resolved.arg().redacted().contains("s3cr3t"));

        let token = map(&[(
            "github.com",
            GitSecret::PlainToken {
                token: "ghp_tok".to_string(),
                username: "git".to_string(),
            },
        )]);
        let resolved = git_url_for(url, &token, ssh_dir, None).await.unwrap();
        assert_eq!(
            resolved.arg().plaintext(),
            "https://git:ghp_tok@github.com/ceph/ceph"
        );
        assert!(!resolved.arg().redacted().contains("ghp_tok"));
    }

    #[tokio::test]
    async fn vault_git_secret_without_a_vault_is_an_error() {
        // A matched vault-backed secret needs a Vault client; with `None` it
        // errors rather than silently cloning the bare URL.
        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();
        let secrets = map(&[(
            "github.com",
            GitSecret::VaultHttps {
                key: "git/ceph".to_string(),
                username: "username".to_string(),
                password: "password".to_string(),
            },
        )]);
        let err = git_url_for("https://github.com/ceph/ceph", &secrets, ssh_dir, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::VaultRequired), "{err}");
    }

    /// End-to-end `vault-https` resolution against a live Vault. Seeds a
    /// `ces-kv` secret whose `username`/`password` fields are the values folded
    /// into the URL, then resolves through a Vault-backed `git_url_for`. Ignored
    /// — needs a running dev server (same setup as the `vault.rs` read test):
    ///
    /// ```text
    /// vault server -dev -dev-root-token-id=root
    /// export VAULT_ADDR=http://127.0.0.1:8200 VAULT_TOKEN=root
    /// vault secrets enable -path=ces-kv kv-v2      # one-time
    /// cargo test -p cbscore --lib -- --ignored vault_https_secret_resolves
    /// ```
    #[tokio::test]
    #[ignore = "requires a live Vault dev server with a ces-kv kv-v2 mount"]
    async fn vault_https_secret_resolves_credentials_from_vault() {
        use cbscore_types::VaultConfig;
        use vaultrs::client::{VaultClient, VaultClientSettingsBuilder};

        let addr = std::env::var("VAULT_ADDR").expect("VAULT_ADDR set for the dev server");
        let token = std::env::var("VAULT_TOKEN").expect("VAULT_TOKEN set to the dev root token");

        // Seed the secret: the field *values* are the creds folded into the URL.
        let settings = VaultClientSettingsBuilder::default()
            .address(&addr)
            .token(&token)
            .build()
            .expect("seed settings");
        let seed = VaultClient::new(settings).expect("seed client");
        let data = BTreeMap::from([
            ("username".to_string(), "git".to_string()),
            ("password".to_string(), "s3cr3t".to_string()),
        ]);
        vaultrs::kv2::set(&seed, "ces-kv", "git/ceph", &data)
            .await
            .expect("seed ces-kv/git/ceph");

        // `username`/`password` here are field *names* in that secret.
        let secrets = map(&[(
            "github.com",
            GitSecret::VaultHttps {
                key: "git/ceph".to_string(),
                username: "username".to_string(),
                password: "password".to_string(),
            },
        )]);
        let vault = Vault::from_config(&VaultConfig {
            schema_version: 1,
            vault_addr: addr,
            auth_user: None,
            auth_approle: None,
            auth_token: Some(token),
        })
        .expect("vault from config");

        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();
        let resolved = git_url_for(
            "https://github.com/ceph/ceph",
            &secrets,
            ssh_dir,
            Some(&vault),
        )
        .await
        .expect("vault-https resolution");

        assert_eq!(
            resolved.arg().plaintext(),
            "https://git:s3cr3t@github.com/ceph/ceph"
        );
        assert!(!resolved.arg().redacted().contains("s3cr3t"));
    }

    #[test]
    fn write_ssh_material_writes_key_config_and_cleans_up_on_drop() {
        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();

        let (alias, guard) = write_ssh_material(
            ssh_dir,
            "github.com",
            "22",
            "git",
            "PRIVATE-KEY-MATERIAL",
            "github.com ssh-rsa AAAAFAKEHOSTKEY\n",
            "ceph/ceph",
        )
        .unwrap();

        // The alias is `<10 letters>:<repo>`.
        let remote = alias
            .strip_suffix(":ceph/ceph")
            .expect("alias ends with repo");
        assert_eq!(remote.len(), 10);
        assert!(remote.chars().all(|c| c.is_ascii_alphabetic()));

        // The key file is 0600 and carries the key plus a trailing newline.
        assert!(guard.key_path.as_str().ends_with(&format!("id_{remote}")));
        let mode = std::fs::metadata(&guard.key_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "key must be private");
        assert_eq!(
            std::fs::read_to_string(&guard.key_path).unwrap(),
            "PRIVATE-KEY-MATERIAL\n"
        );

        // The config has the Host alias; known_hosts has the scanned key.
        let config = std::fs::read_to_string(ssh_dir.join("config")).unwrap();
        assert!(config.contains(&format!("Host {remote}")), "{config}");
        assert!(config.contains("IdentityFile"), "{config}");
        let known = std::fs::read_to_string(ssh_dir.join("known_hosts")).unwrap();
        assert!(known.contains("AAAAFAKEHOSTKEY"), "{known}");

        // Dropping the guard removes the private key (the config/known_hosts
        // entries persist, as in Python).
        let key_path = guard.key_path.clone();
        drop(guard);
        assert!(!key_path.exists(), "key removed on drop");
    }

    /// The full plain-SSH path of `git_url_for`: scan the host key, write the
    /// key/config/known_hosts, and yield a `<alias>:<repo>` clone target whose
    /// key the resolved URL's guard removes on drop. The offline half is covered
    /// above; this exercises the `ssh-keyscan` step too, so it is ignored by
    /// default — it needs the network and the `ssh-keyscan` binary. Run with
    /// `cargo test -p cbscore --lib -- --ignored plain_ssh`.
    #[tokio::test]
    #[ignore = "requires network and ssh-keyscan (scans a real host key)"]
    async fn plain_ssh_secret_materializes_a_key_and_alias() {
        let ssh = tempfile::tempdir().unwrap();
        let ssh_dir = Utf8Path::from_path(ssh.path()).unwrap();
        let secrets = map(&[(
            "github.com",
            GitSecret::PlainSsh {
                ssh_key: "PRIVATE-KEY-MATERIAL".to_string(),
                username: "git".to_string(),
            },
        )]);

        let resolved = git_url_for("https://github.com/ceph/ceph", &secrets, ssh_dir, None)
            .await
            .expect("plain-ssh resolution");

        // The clone target is `<10 letters>:<repo>`, not the original URL.
        let alias = resolved.arg().plaintext().to_string();
        let remote = alias
            .strip_suffix(":ceph/ceph")
            .expect("alias ends with the repo path");
        assert_eq!(remote.len(), 10);
        assert!(remote.chars().all(|c| c.is_ascii_alphabetic()));

        // The private key landed at id_<alias> with mode 0600.
        let key_path = ssh_dir.join(format!("id_{remote}"));
        assert!(key_path.exists(), "key written");
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key must be private");

        // The config got the Host alias and known_hosts the scanned host key.
        let config = std::fs::read_to_string(ssh_dir.join("config")).unwrap();
        assert!(config.contains(&format!("Host {remote}")), "{config}");
        let known = std::fs::read_to_string(ssh_dir.join("known_hosts")).unwrap();
        assert!(
            !known.trim().is_empty(),
            "host key scanned into known_hosts"
        );

        // Dropping the resolved URL removes the temporary key.
        drop(resolved);
        assert!(
            !key_path.exists(),
            "key removed when the resolved url drops"
        );
    }
}
