// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

//! Component tarball validation and unpacking.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Errors during component tarball handling.
#[derive(Debug)]
pub enum ComponentError {
    /// SHA-256 of the tarball does not match the expected value.
    IntegrityFailed { expected: String, actual: String },
    /// Failed to create the temporary directory.
    TempDir(std::io::Error),
    /// Failed to unpack the tar.gz archive.
    Unpack(std::io::Error),
}

impl std::fmt::Display for ComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrityFailed { expected, actual } => {
                write!(
                    f,
                    "component integrity check failed: expected sha256={expected}, got {actual}"
                )
            }
            Self::TempDir(err) => write!(f, "failed to create temp directory: {err}"),
            Self::Unpack(err) => write!(f, "failed to unpack component tarball: {err}"),
        }
    }
}

impl std::error::Error for ComponentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IntegrityFailed { .. } => None,
            Self::TempDir(err) | Self::Unpack(err) => Some(err),
        }
    }
}

/// Validate the SHA-256 hash of `tarball_bytes` against `expected_sha256`,
/// then unpack the tar.gz archive into a new subdirectory under `temp_dir`.
///
/// Returns the path to the unpacked directory.
pub fn validate_and_unpack(
    tarball_bytes: &[u8],
    expected_sha256: &str,
    temp_dir: &Path,
) -> Result<PathBuf, ComponentError> {
    // 1. Compute SHA-256 and compare.
    let mut hasher = Sha256::new();
    hasher.update(tarball_bytes);
    let actual = format!("{:x}", hasher.finalize());

    if actual != expected_sha256 {
        return Err(ComponentError::IntegrityFailed {
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    // 2. Create a subdirectory for unpacking.
    let unpack_dir = temp_dir.join(format!("component-{}", &actual[..12]));
    std::fs::create_dir_all(&unpack_dir).map_err(ComponentError::TempDir)?;

    // 3. Unpack tar.gz.
    let decoder = flate2::read::GzDecoder::new(tarball_bytes);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&unpack_dir)
        .map_err(ComponentError::Unpack)?;

    Ok(unpack_dir)
}

/// Remove the unpacked component directory. Logs a warning on failure rather
/// than returning an error, since cleanup failures are non-fatal.
pub fn cleanup(path: &Path) {
    if let Err(err) = std::fs::remove_dir_all(path) {
        tracing::warn!(path = %path.display(), %err, "failed to clean up component directory");
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Create a minimal tar.gz in memory containing a single file.
    fn make_test_tarball(filename: &str, content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, filename, content).unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        // Compress with gzip.
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn validate_and_unpack_success() {
        let tarball = make_test_tarball("test.txt", b"hello world");
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result = validate_and_unpack(&tarball, &hash, tmp.path());
        assert!(result.is_ok());

        let unpack_dir = result.unwrap();
        assert!(unpack_dir.join("test.txt").exists());
    }

    #[test]
    fn validate_and_unpack_integrity_failure() {
        let tarball = make_test_tarball("test.txt", b"hello world");
        let tmp = tempfile::tempdir().unwrap();

        let result = validate_and_unpack(&tarball, "0000000000000000", tmp.path());
        assert!(matches!(
            result,
            Err(ComponentError::IntegrityFailed { .. })
        ));
    }

    #[test]
    fn cleanup_nonexistent_path_does_not_panic() {
        cleanup(Path::new("/tmp/nonexistent-cbsd-test-path-12345"));
    }
}
