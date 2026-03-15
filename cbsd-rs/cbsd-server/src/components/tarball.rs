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

//! Pack a component directory into a gzip-compressed tar archive.

use std::io;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

/// Pack the component directory into a gzip-compressed tar archive.
///
/// Returns `(tar_gz_bytes, sha256_hex)` where `sha256_hex` is the hex-encoded
/// SHA-256 digest of the final gzip bytes. The archive entries are stored
/// relative to `component_name/` as the top-level directory.
pub fn pack_component(
    component_dir: &Path,
    component_name: &str,
) -> Result<(Vec<u8>, String), io::Error> {
    let buf = Vec::new();
    let encoder = GzEncoder::new(buf, Compression::fast());
    let mut archive = tar::Builder::new(encoder);

    // Append the entire component directory under the component name prefix.
    archive.append_dir_all(component_name, component_dir)?;

    // Finish the tar archive and then the gzip stream.
    let encoder = archive.into_inner()?;
    let gz_bytes = encoder.finish()?;

    // Compute SHA-256 over the final gzip bytes.
    let mut hasher = Sha256::new();
    hasher.update(&gz_bytes);
    let hash = hasher.finalize();
    let hex = hex_encode(&hash);

    Ok((gz_bytes, hex))
}

/// Encode a byte slice as lowercase hex.
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn pack_and_verify_sha256() {
        let tmp = std::env::temp_dir().join("cbsd-test-tarball");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("file.txt"), b"hello world").unwrap();
        fs::write(tmp.join("sub/nested.txt"), b"nested content").unwrap();

        let (gz_bytes, sha256_hex) = pack_component(&tmp, "test-component").unwrap();

        // Verify the bytes are non-empty gzip (magic bytes 1f 8b)
        assert!(gz_bytes.len() > 20);
        assert_eq!(gz_bytes[0], 0x1f);
        assert_eq!(gz_bytes[1], 0x8b);

        // Verify SHA-256 is 64 hex chars
        assert_eq!(sha256_hex.len(), 64);

        // Re-hash and verify consistency
        let mut hasher = sha2::Sha256::new();
        hasher.update(&gz_bytes);
        let hash = hasher.finalize();
        assert_eq!(hex_encode(&hash), sha256_hex);

        // Clean up
        let _ = fs::remove_dir_all(&tmp);
    }
}
