// crt — secrets loading.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The git-ignored `crt.secrets.yaml` (design §9): S3 credentials (and, from
//! M2, the Vault address/token + GPG signing-key path). Kept separate from the
//! non-secret `crt.config.yaml`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Secrets {
    #[serde(default)]
    pub s3: Option<S3Secrets>,
    // `vault` is added in M2 (release signing key).
}

#[derive(Debug, Deserialize)]
pub struct S3Secrets {
    pub access_key_id: String,
    pub secret_access_key: String,
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
}
