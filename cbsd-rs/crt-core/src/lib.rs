// CRT core — content-addressing, manifest, and sealing primitives (no IO).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pure domain primitives for CRT. This crate performs no IO and is
//! runtime-agnostic; see the design doc (`docs/crt/design`) §3–§7.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Hasher};
use thiserror::Error;

pub mod annotations;
pub mod manifest;
pub mod materialize;
pub mod meta;
pub mod notes;
pub mod sbom;
pub mod seal;

pub use annotations::{
    ANNOTATIONS_SCHEMA_VERSION, Applicability, PatchAnnotations, VersionQuery, VersionSpec,
    applies_to_matches, parse_version_query, parse_version_spec,
};

pub use manifest::{
    ArmoredSignature, Band, Blast, Branding, Conflict, Coverage, DataStructureChange, Draft,
    Justification, JustificationKind, KnownIssue, Lifecycle, Manifest, ManifestEntry, PatchStatus,
    ReleaseHeader, ReleaseKey, ReleaseRecord, RenderSpec, Risk, SCHEMA_VERSION, Visibility,
    canonical_json, digest, upstream_weight,
};
pub use materialize::{
    MATERIALIZATION_RECORD_VERSION, MaterializationRecord, MaterializedPatch,
    PublicPatchProvenance, PublicProvenance, source_tree_digest,
};
pub use meta::{Identity, PatchMeta, Provenance, UpstreamPrState, cherry_picked_from};
pub use notes::{RENDER_MINIJINJA_VERSION, check_render_version, render_notes};
pub use sbom::build_sbom;
pub use seal::{sign_manifest, verify_manifest};

/// Errors produced by `crt-core`.
#[derive(Debug, Error)]
pub enum CrtCoreError {
    /// A string was not a valid lowercase-hex SHA-256.
    #[error("invalid sha256: {0:?}")]
    InvalidSha256(String),
    /// A `--ceph-version` value was not a valid ceph version or line
    /// (design §5/§7): non-numeric, too few components, or a pre-release tag
    /// on a `major.minor` line.
    #[error("invalid ceph version: {0:?}")]
    InvalidVersion(String),
    /// RFC 8785 canonical-JSON serialization failed (design §6).
    #[error("canonical json: {0}")]
    Canonical(String),
    /// An OpenPGP sign/verify/parse operation failed (design §6).
    #[error("openpgp: {0}")]
    Pgp(String),
    /// A `minijinja` release-notes render failed (design §7.2).
    #[error("notes render: {0}")]
    Render(String),
    /// The sealed manifest pins a `minijinja` version this build does not link
    /// (design §7.2): refuse to silently re-render with a different engine.
    #[error("minijinja version mismatch: manifest sealed {sealed}, this build links {linked}")]
    RenderVersionMismatch { sealed: String, linked: String },
    /// CycloneDX SBOM serialization failed (design §7.1).
    #[error("sbom serialization: {0}")]
    Sbom(String),
}

/// A SHA-256 content address, serialized as a lowercase-hex string.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256([u8; 32]);

impl Sha256 {
    /// Hash raw bytes into a content address.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256Hasher::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// Render as a lowercase-hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl std::fmt::Display for Sha256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl std::fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sha256({})", self.to_hex())
    }
}

impl TryFrom<String> for Sha256 {
    type Error = CrtCoreError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let bytes = hex::decode(&s).map_err(|_| CrtCoreError::InvalidSha256(s.clone()))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CrtCoreError::InvalidSha256(s))?;
        Ok(Self(arr))
    }
}

impl From<Sha256> for String {
    fn from(h: Sha256) -> Self {
        h.to_hex()
    }
}

/// The content address of a patch blob: `sha256` of the exact stored bytes.
///
/// A byte-exact artifact address (design §4). Logical equivalence across
/// rebase / re-export is handled separately by `git patch-id --stable`.
#[must_use]
pub fn blob_hash(raw: &[u8]) -> Sha256 {
    Sha256::of(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_hash_is_deterministic_and_byte_exact() {
        let a = blob_hash(b"hello");
        let b = blob_hash(b"hello");
        let c = blob_hash(b"hello!");
        assert_eq!(a, b);
        assert_ne!(a, c);
        // Known SHA-256 of "hello".
        assert_eq!(
            a.to_hex(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_round_trips_through_hex() {
        let h = blob_hash(b"content");
        let s = h.to_hex();
        let back = Sha256::try_from(s.clone()).expect("valid hex round-trips");
        assert_eq!(h, back);
        assert_eq!(back.to_hex(), s);
    }

    #[test]
    fn invalid_hex_is_rejected() {
        assert!(Sha256::try_from("nothex".to_string()).is_err());
        assert!(Sha256::try_from("ab".to_string()).is_err()); // wrong length
    }
}
