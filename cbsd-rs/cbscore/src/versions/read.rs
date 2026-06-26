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

//! Reading a version descriptor from disk (design 002/006). The wire type and
//! its pure `parse` live in `cbscore-types`; the "descriptor file is absent" IO
//! error lives here, with the subsystem that performs the IO (design 002).
//!
//! First consumers: the host runner's input validation and the in-container
//! `runner build` entry (design 009/010), which both need the descriptor.

use camino::{Utf8Path, Utf8PathBuf};
use cbscore_types::VersionDescriptor;

/// An error reading a version descriptor.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The descriptor file does not exist (Python's `NoSuchVersionDescriptor`).
    #[error("version descriptor '{path}' does not exist")]
    NotFound { path: Utf8PathBuf },
    /// The descriptor file could not be read.
    #[error("error reading version descriptor '{path}'")]
    Read {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The descriptor bytes fail to parse or validate (`InvalidVersionDescriptor`
    /// / `UnknownSchemaVersion`, design 002).
    #[error(transparent)]
    Invalid(cbscore_types::Error),
}

/// Read and validate a version descriptor from `path`.
pub async fn read_descriptor(path: &Utf8Path) -> Result<VersionDescriptor, ReadError> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReadError::NotFound {
                path: path.to_owned(),
            });
        }
        Err(source) => {
            return Err(ReadError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    VersionDescriptor::parse(&raw, Some(path)).map_err(ReadError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_a_written_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path()).unwrap().join("20.2.1.json");
        let raw = r#"{
            "version": "20.2.1",
            "title": "Release version 20.2.1",
            "signed_off_by": {"user": "Jane", "email": "jane@example.com"},
            "image": {"registry": "harbor.clyso.com", "name": "ces/ceph/ceph", "tag": "20.2.1"},
            "components": [{"name": "ceph", "repo": "https://github.com/ceph/ceph", "ref": "v20.2.1"}],
            "distro": "rockylinux:9",
            "el_version": 9
        }"#;
        tokio::fs::write(&path, raw).await.unwrap();
        let desc = read_descriptor(&path).await.unwrap();
        assert_eq!(desc.version, "20.2.1");
        assert_eq!(desc.distro, "rockylinux:9");
    }

    #[tokio::test]
    async fn missing_descriptor_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path()).unwrap().join("nope.json");
        assert!(matches!(
            read_descriptor(&path).await,
            Err(ReadError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn malformed_descriptor_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path()).unwrap().join("bad.json");
        tokio::fs::write(&path, "{not json}").await.unwrap();
        assert!(matches!(
            read_descriptor(&path).await,
            Err(ReadError::Invalid(_))
        ));
    }
}
