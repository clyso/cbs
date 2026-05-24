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
//!
//! Per audit-rem D5 (Phase 2): the unpack loop rejects path traversal,
//! out-of-root symlink targets (two-phase logical + real-path check),
//! and non-regular/non-dir/non-link entry types; a decompression cap
//! defends against gzip-bomb attacks.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use tar::EntryType;

/// Default cap on total uncompressed tarball bytes. 256 MiB is roughly
/// 4 orders of magnitude above today's typical component (~2 KiB), so
/// it accommodates substantial future growth while still defending
/// against a gzip bomb expanding to multi-GiB on the worker host.
pub const DEFAULT_MAX_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;

/// Errors during component tarball handling.
#[derive(Debug)]
pub enum ComponentError {
    /// SHA-256 of the tarball does not match the expected value.
    IntegrityFailed { expected: String, actual: String },
    /// Failed to create the temporary directory.
    TempDir(std::io::Error),
    /// Failed to unpack the tar.gz archive.
    Unpack(std::io::Error),
    /// Tarball entry violated containment: path traversal, absolute or
    /// escaping symlink target, hardlink resolving outside the unpack
    /// root, or a TOCTOU symlink mutation detected by the phase-2
    /// real-path walk.
    Containment(String),
    /// Tarball entry has a rejected entry type (device, fifo, block,
    /// char, or any other non-regular/non-dir/non-link type).
    RejectedEntryType(String),
    /// Decompression cap exceeded — either a single entry's declared
    /// size exceeded the cap, or the cumulative uncompressed stream
    /// exceeded it.
    CapExceeded(u64),
}

impl std::fmt::Display for ComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrityFailed { expected, actual } => write!(
                f,
                "component integrity check failed: expected sha256={expected}, got {actual}"
            ),
            Self::TempDir(err) => write!(f, "failed to create temp directory: {err}"),
            Self::Unpack(err) => write!(f, "failed to unpack component tarball: {err}"),
            Self::Containment(msg) => write!(f, "tarball containment violation: {msg}"),
            Self::RejectedEntryType(msg) => {
                write!(f, "tarball contains rejected entry type: {msg}")
            }
            Self::CapExceeded(cap) => {
                write!(f, "tarball decompression cap of {cap} bytes exceeded")
            }
        }
    }
}

impl std::error::Error for ComponentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TempDir(err) | Self::Unpack(err) => Some(err),
            _ => None,
        }
    }
}

/// Custom error type carried inside [`std::io::Error`] when the
/// [`LimitedReader`] cap is exhausted. Downcasting from the tar crate's
/// wrapped error path lets us report a clean `CapExceeded` to callers
/// rather than a generic IO error.
#[derive(Debug)]
struct CapExceededError {
    cap: u64,
}

impl std::fmt::Display for CapExceededError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "decompression cap of {} bytes exceeded", self.cap)
    }
}

impl std::error::Error for CapExceededError {}

/// Reader that returns a [`CapExceededError`] once `cap` bytes have
/// been read from the inner stream. The remaining counter is set to
/// `cap + 1` so an archive whose uncompressed size is *exactly* the
/// cap reads cleanly to its trailing zero-block.
struct LimitedReader<R> {
    inner: R,
    remaining: u64,
    cap: u64,
}

impl<R: Read> LimitedReader<R> {
    fn new(inner: R, cap: u64) -> Self {
        Self {
            inner,
            remaining: cap.saturating_add(1),
            cap,
        }
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                CapExceededError { cap: self.cap },
            ));
        }
        let max = std::cmp::min(buf.len() as u64, self.remaining) as usize;
        let n = self.inner.read(&mut buf[..max])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Walk an [`std::io::Error`]'s `get_ref()` and `source()` chain
/// looking for a [`CapExceededError`]. The tar crate wraps our IO
/// error in `TarError`, so we have to traverse one or two levels.
fn extract_cap_exceeded(err: &std::io::Error) -> Option<u64> {
    let inner = err.get_ref()?;
    if let Some(cap_err) = inner.downcast_ref::<CapExceededError>() {
        return Some(cap_err.cap);
    }
    let mut next = inner.source();
    while let Some(e) = next {
        if let Some(cap_err) = e.downcast_ref::<CapExceededError>() {
            return Some(cap_err.cap);
        }
        next = e.source();
    }
    None
}

/// Logically clean `path` by collapsing `.` and `..` components
/// without consulting the filesystem. Returns `None` if a `..` pops
/// past the start (escape) or the path contains a Windows drive
/// prefix. A leading `/` is preserved; a leading `..` against a
/// relative path is treated as an escape.
fn lexical_clean(path: &Path) -> Option<PathBuf> {
    let mut stack: Vec<std::ffi::OsString> = Vec::new();
    let mut have_root = false;

    for comp in path.components() {
        match comp {
            Component::Prefix(_) => return None,
            Component::RootDir => {
                have_root = true;
                stack.clear();
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                if stack.is_empty() {
                    return None;
                }
                stack.pop();
            }
            Component::Normal(s) => stack.push(s.to_os_string()),
        }
    }

    let mut result = if have_root {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for s in stack {
        result.push(s);
    }
    Some(result)
}

/// Phase 1 (logical containment): return the absolute path inside
/// `unpack_root` if `rel` resolves there lexically. Returns `None` on
/// any escape (absolute path, `..` past root, Windows prefix).
fn logical_normalize_within(unpack_root: &Path, rel: &Path) -> Option<PathBuf> {
    if rel.is_absolute() {
        return None;
    }
    let combined = unpack_root.join(rel);
    let cleaned = lexical_clean(&combined)?;
    if cleaned == unpack_root || cleaned.starts_with(unpack_root) {
        Some(cleaned)
    } else {
        None
    }
}

/// Phase 2 (real-path containment): canonicalize the parent of
/// `entry_path_absolute` and confirm it stays inside `unpack_root`'s
/// canonical form. Defends against TOCTOU symlink swaps and any
/// implementation bug in phase 1 that admits a path the on-disk reality
/// disagrees with.
fn verify_parent_realpath_under(
    unpack_root_real: &Path,
    entry_path_absolute: &Path,
) -> std::io::Result<()> {
    let parent = entry_path_absolute.parent().unwrap_or(unpack_root_real);
    let parent_real = parent.canonicalize()?;
    if parent_real == unpack_root_real || parent_real.starts_with(unpack_root_real) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "parent of '{}' resolves to '{}' which escapes unpack root '{}'",
                entry_path_absolute.display(),
                parent_real.display(),
                unpack_root_real.display(),
            ),
        ))
    }
}

/// Validate the SHA-256 of `tarball_bytes` against `expected_sha256`,
/// then unpack the tar.gz archive into a new subdirectory under
/// `temp_dir`, enforcing containment + a `max_uncompressed_bytes` cap.
///
/// Returns the path to the unpacked directory.
pub fn validate_and_unpack(
    tarball_bytes: &[u8],
    expected_sha256: &str,
    temp_dir: &Path,
    max_uncompressed_bytes: u64,
) -> Result<PathBuf, ComponentError> {
    // 1. SHA-256.
    let mut hasher = Sha256::new();
    hasher.update(tarball_bytes);
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_sha256 {
        return Err(ComponentError::IntegrityFailed {
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    // 2. Create per-build subdirectory and resolve its canonical form
    //    for the real-path containment baseline.
    let unpack_dir = temp_dir.join(format!("component-{}", &actual[..12]));
    std::fs::create_dir_all(&unpack_dir).map_err(ComponentError::TempDir)?;
    let unpack_root_real = unpack_dir.canonicalize().map_err(ComponentError::TempDir)?;

    // 3. Wrap the decoder in our capped reader, then iterate entries.
    let decoder = flate2::read::GzDecoder::new(tarball_bytes);
    let capped = LimitedReader::new(decoder, max_uncompressed_bytes);
    let mut archive = tar::Archive::new(capped);

    let entries = archive
        .entries()
        .map_err(|e| match extract_cap_exceeded(&e) {
            Some(cap) => ComponentError::CapExceeded(cap),
            None => ComponentError::Unpack(e),
        })?;

    for entry_result in entries {
        let mut entry = entry_result.map_err(|e| match extract_cap_exceeded(&e) {
            Some(cap) => ComponentError::CapExceeded(cap),
            None => ComponentError::Unpack(e),
        })?;
        unpack_one(&mut entry, &unpack_root_real, max_uncompressed_bytes)?;
    }

    Ok(unpack_dir)
}

/// Process one tar entry: validate containment, then write to disk.
fn unpack_one<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    unpack_root_real: &Path,
    max_uncompressed_bytes: u64,
) -> Result<(), ComponentError> {
    let entry_type = entry.header().entry_type();
    // PAX-aware path (the tar crate applies extended-header overrides
    // before this returns, so the check below operates on the effective
    // name — never the raw POSIX 100-byte field).
    let entry_path = entry.path().map_err(ComponentError::Unpack)?.into_owned();

    // Phase 1: logical containment of the entry's own path.
    let dest = logical_normalize_within(unpack_root_real, &entry_path).ok_or_else(|| {
        ComponentError::Containment(format!(
            "entry path '{}' escapes unpack root (logical check)",
            entry_path.display()
        ))
    })?;

    // Per-entry size cap (defends against a malformed entry that
    // declares a header size larger than the cap).
    if entry.size() > max_uncompressed_bytes {
        return Err(ComponentError::CapExceeded(max_uncompressed_bytes));
    }

    match entry_type {
        EntryType::Directory => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(ComponentError::Unpack)?;
            }
            // Phase 2: confirm the on-disk parent path still resolves
            // inside the unpack root before creating a directory.
            // Design D5: the phase-2 check applies to every entry —
            // symlink, regular file, directory, or hardlink.
            verify_parent_realpath_under(unpack_root_real, &dest)
                .map_err(|e| ComponentError::Containment(e.to_string()))?;
            std::fs::create_dir_all(&dest).map_err(ComponentError::Unpack)?;
        }

        EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(ComponentError::Unpack)?;
            }
            // Phase 2: confirm the on-disk parent path still resolves
            // inside the unpack root before writing.
            verify_parent_realpath_under(unpack_root_real, &dest)
                .map_err(|e| ComponentError::Containment(e.to_string()))?;
            let mut out = std::fs::File::create(&dest).map_err(ComponentError::Unpack)?;
            std::io::copy(entry, &mut out).map_err(|e| match extract_cap_exceeded(&e) {
                Some(cap) => ComponentError::CapExceeded(cap),
                None => ComponentError::Unpack(e),
            })?;
        }

        EntryType::Symlink => {
            let target = entry
                .link_name()
                .map_err(ComponentError::Unpack)?
                .ok_or_else(|| {
                    ComponentError::Containment(format!(
                        "symlink entry '{}' has no link target",
                        entry_path.display()
                    ))
                })?
                .into_owned();

            // Phase 1: symlink target containment. Absolute targets are
            // rejected unconditionally; relative targets are resolved
            // against the symlink's containing directory and lexically
            // normalized.
            if target.is_absolute() {
                return Err(ComponentError::Containment(format!(
                    "symlink '{}' has absolute target '{}'",
                    entry_path.display(),
                    target.display()
                )));
            }
            let link_dir = dest.parent().unwrap_or(unpack_root_real);
            let logical_target = link_dir.join(&target);
            let cleaned_target = lexical_clean(&logical_target).ok_or_else(|| {
                ComponentError::Containment(format!(
                    "symlink '{}' target '{}' escapes unpack root (logical check)",
                    entry_path.display(),
                    target.display()
                ))
            })?;
            if cleaned_target != unpack_root_real && !cleaned_target.starts_with(unpack_root_real) {
                return Err(ComponentError::Containment(format!(
                    "symlink '{}' target '{}' resolves to '{}' which escapes unpack root",
                    entry_path.display(),
                    target.display(),
                    cleaned_target.display()
                )));
            }

            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(ComponentError::Unpack)?;
            }
            // Phase 2: confirm the on-disk parent path still resolves
            // inside the unpack root before symlinking.
            verify_parent_realpath_under(unpack_root_real, &dest)
                .map_err(|e| ComponentError::Containment(e.to_string()))?;
            std::os::unix::fs::symlink(&target, &dest).map_err(ComponentError::Unpack)?;
        }

        EntryType::Link => {
            // Hardlink targets in tar are paths relative to the unpack
            // root (they reference another entry already in the
            // archive). Validate the same way as a regular path.
            let target = entry
                .link_name()
                .map_err(ComponentError::Unpack)?
                .ok_or_else(|| {
                    ComponentError::Containment(format!(
                        "hardlink entry '{}' has no link target",
                        entry_path.display()
                    ))
                })?
                .into_owned();

            let resolved_target =
                logical_normalize_within(unpack_root_real, &target).ok_or_else(|| {
                    ComponentError::Containment(format!(
                        "hardlink '{}' target '{}' escapes unpack root (logical check)",
                        entry_path.display(),
                        target.display()
                    ))
                })?;

            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(ComponentError::Unpack)?;
            }
            verify_parent_realpath_under(unpack_root_real, &dest)
                .map_err(|e| ComponentError::Containment(e.to_string()))?;
            std::fs::hard_link(&resolved_target, &dest).map_err(ComponentError::Unpack)?;
        }

        // Any other entry type (Char, Block, Fifo, XHeader, XGlobalHeader,
        // GNULongName, GNULongLink, __Nonexhaustive) is rejected. The
        // tar crate normally consumes XHeader/XGlobalHeader/GNULong*
        // internally before yielding an entry, so reaching these here
        // implies an attacker-crafted archive.
        EntryType::Char => {
            return Err(ComponentError::RejectedEntryType(format!(
                "character device '{}'",
                entry_path.display()
            )));
        }
        EntryType::Block => {
            return Err(ComponentError::RejectedEntryType(format!(
                "block device '{}'",
                entry_path.display()
            )));
        }
        EntryType::Fifo => {
            return Err(ComponentError::RejectedEntryType(format!(
                "fifo '{}'",
                entry_path.display()
            )));
        }
        other => {
            return Err(ComponentError::RejectedEntryType(format!(
                "unsupported tar entry type {:?} at '{}'",
                other,
                entry_path.display()
            )));
        }
    }

    Ok(())
}

/// Remove the unpacked component directory. Logs a warning on failure
/// rather than returning an error, since cleanup failures are
/// non-fatal.
pub fn cleanup(path: &Path) {
    if let Err(err) = std::fs::remove_dir_all(path) {
        tracing::warn!(path = %path.display(), %err, "failed to clean up component directory");
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::symlink as unix_symlink;

    use super::*;

    /// Construct a tar.gz blob from a closure that mutates the
    /// underlying `tar::Builder`. The closure is responsible for
    /// emitting headers/entries.
    fn build_tarball<F>(make_entries: F) -> Vec<u8>
    where
        F: FnOnce(&mut tar::Builder<Vec<u8>>),
    {
        let mut builder = tar::Builder::new(Vec::new());
        make_entries(&mut builder);
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn make_test_tarball(filename: &str, content: &[u8]) -> Vec<u8> {
        build_tarball(|builder| {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, filename, content).unwrap();
        })
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    fn append_symlink(builder: &mut tar::Builder<Vec<u8>>, name: &str, target: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header
            .set_path(name)
            .unwrap_or_else(|e| panic!("set_path failed: {e}"));
        header
            .set_link_name(target)
            .unwrap_or_else(|e| panic!("set_link_name failed: {e}"));
        header.set_cksum();
        builder
            .append(&header, std::io::empty())
            .unwrap_or_else(|e| panic!("append symlink failed: {e}"));
    }

    fn append_hardlink(builder: &mut tar::Builder<Vec<u8>>, name: &str, target: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(EntryType::Link);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_path(name).unwrap();
        header.set_link_name(target).unwrap();
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
    }

    fn append_device(builder: &mut tar::Builder<Vec<u8>>, name: &str, entry_type: EntryType) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(entry_type);
        header.set_size(0);
        header.set_mode(0o666);
        header.set_path(name).unwrap();
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
    }

    fn append_file(builder: &mut tar::Builder<Vec<u8>>, name: &str, content: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, content).unwrap();
    }

    // ---------------------------------------------------------------
    // Pre-existing behaviour
    // ---------------------------------------------------------------

    #[test]
    fn validate_and_unpack_success() {
        let tarball = make_test_tarball("test.txt", b"hello world");
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        let unpack_dir = result.expect("happy path must succeed");
        assert!(unpack_dir.join("test.txt").exists());
    }

    #[test]
    fn validate_and_unpack_integrity_failure() {
        let tarball = make_test_tarball("test.txt", b"hello world");
        let tmp = tempfile::tempdir().unwrap();

        let result = validate_and_unpack(
            &tarball,
            "0000000000000000",
            tmp.path(),
            DEFAULT_MAX_UNCOMPRESSED_BYTES,
        );
        assert!(matches!(
            result,
            Err(ComponentError::IntegrityFailed { .. })
        ));
    }

    #[test]
    fn cleanup_nonexistent_path_does_not_panic() {
        cleanup(Path::new("/tmp/nonexistent-cbsd-test-path-12345"));
    }

    // ---------------------------------------------------------------
    // D5: containment (happy path)
    // ---------------------------------------------------------------

    #[test]
    fn accepts_legitimate_same_directory_symlink() {
        // Mirrors the real-world `components/ceph/containers/v20.3 -> ./v20.2`
        // alias that must keep working post-D5.
        let tarball = build_tarball(|b| {
            append_file(b, "containers/v20.2/cbs.component.yaml", b"name: foo\n");
            append_symlink(b, "containers/v20.3", "./v20.2");
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let unpack_dir =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES)
                .expect("legitimate same-dir symlink must unpack");

        let link = unpack_dir.join("containers/v20.3");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        // The link target was written verbatim; reading through it
        // should reach the real file.
        assert!(
            unpack_dir
                .join("containers/v20.3/cbs.component.yaml")
                .exists()
        );
    }

    #[test]
    fn accepts_symlink_chain_internal() {
        // a -> b, b -> c, regular file c/file. Each link's logical
        // normalization stays inside the unpack root.
        let tarball = build_tarball(|b| {
            append_file(b, "c/file", b"contents\n");
            append_symlink(b, "b", "c");
            append_symlink(b, "a", "b");
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let unpack_dir =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES)
                .expect("benign symlink chain must unpack");

        // Reading through both links resolves to the real file.
        assert!(unpack_dir.join("a/file").exists());
    }

    // ---------------------------------------------------------------
    // D5: containment (attack vectors)
    // ---------------------------------------------------------------

    #[test]
    fn rejects_absolute_symlink_target() {
        let tarball = build_tarball(|b| {
            append_symlink(b, "link", "/etc/passwd");
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(
            matches!(result, Err(ComponentError::Containment(_))),
            "expected Containment, got {result:?}"
        );
    }

    #[test]
    fn rejects_relative_escape_symlink_target() {
        let tarball = build_tarball(|b| {
            append_symlink(b, "link", "../../etc/passwd");
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(
            matches!(result, Err(ComponentError::Containment(_))),
            "expected Containment, got {result:?}"
        );
    }

    #[test]
    fn rejects_exact_dotdot_symlink_target() {
        // Symlink target is exactly `..`. Phase 1 lexical normalization
        // of `unpack_root/(link's parent: empty)/..` resolves to one
        // level above the unpack root and must be rejected.
        let tarball = build_tarball(|b| {
            append_symlink(b, "link", "..");
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(
            matches!(result, Err(ComponentError::Containment(_))),
            "expected Containment, got {result:?}"
        );
    }

    #[test]
    fn rejects_pax_overridden_escape_path() {
        // POSIX field carries a benign name; PAX extended header
        // overrides the path to an escape. The tar crate applies PAX
        // before returning the entry path, so our phase-1 check sees
        // the escape.
        let tarball = build_tarball(|b| {
            b.append_pax_extensions([("path", b"../../escape.txt".as_slice())])
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_cksum();
            // The POSIX 100-byte name is the benign `safe.txt`.
            b.append_data(&mut header, "safe.txt", &b"x"[..]).unwrap();
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(
            matches!(result, Err(ComponentError::Containment(_))),
            "expected Containment, got {result:?}"
        );
    }

    #[test]
    fn rejects_device_entry() {
        let tarball = build_tarball(|b| append_device(b, "dev/sda", EntryType::Block));
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(
            matches!(result, Err(ComponentError::RejectedEntryType(_))),
            "expected RejectedEntryType, got {result:?}"
        );
    }

    #[test]
    fn rejects_char_device_entry() {
        let tarball = build_tarball(|b| append_device(b, "dev/null", EntryType::Char));
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(matches!(result, Err(ComponentError::RejectedEntryType(_))));
    }

    #[test]
    fn rejects_fifo_entry() {
        let tarball = build_tarball(|b| append_device(b, "pipe", EntryType::Fifo));
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(matches!(result, Err(ComponentError::RejectedEntryType(_))));
    }

    #[test]
    fn rejects_hardlink_escaping_root() {
        let tarball = build_tarball(|b| {
            append_hardlink(b, "evil", "../../etc/passwd");
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result =
            validate_and_unpack(&tarball, &hash, tmp.path(), DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(
            matches!(result, Err(ComponentError::Containment(_))),
            "expected Containment, got {result:?}"
        );
    }

    // ---------------------------------------------------------------
    // D5: decompression cap
    // ---------------------------------------------------------------

    #[test]
    fn rejects_single_entry_declaring_size_over_cap() {
        // Entry declares a huge size in the header, even though the
        // body is short. Our per-entry size cap catches this.
        let mut header = tar::Header::new_gnu();
        header.set_size(10);
        header.set_mode(0o644);
        header.set_cksum();
        let tarball = build_tarball(|b| {
            // Use append_data so the body matches the size.
            b.append_data(&mut header, "big.bin", &b"xxxxxxxxxx"[..])
                .unwrap();
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        // Use a 5-byte cap so the 10-byte declared size exceeds it.
        let result = validate_and_unpack(&tarball, &hash, tmp.path(), 5);
        assert!(
            matches!(result, Err(ComponentError::CapExceeded(5))),
            "expected CapExceeded(5), got {result:?}"
        );
    }

    #[test]
    fn rejects_gzip_bomb_total_over_cap() {
        // Many small entries that together push the *uncompressed*
        // stream past the cap, even though no individual entry exceeds
        // it. The LimitedReader inside validate_and_unpack catches this
        // via the stream-level cap.
        let payload = vec![0u8; 1024]; // 1 KiB per entry
        let tarball = build_tarball(|b| {
            for i in 0..20 {
                append_file(b, &format!("f{i}"), &payload);
            }
        });
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        // 4 KiB cap; 20 * 1 KiB + tar overhead easily exceeds it.
        let result = validate_and_unpack(&tarball, &hash, tmp.path(), 4 * 1024);
        assert!(
            matches!(result, Err(ComponentError::CapExceeded(_))),
            "expected CapExceeded, got {result:?}"
        );
    }

    #[test]
    fn accepts_archive_exactly_at_cap() {
        // Boundary case from D5: cap == uncompressed archive size →
        // unpack succeeds. The LimitedReader's `remaining = cap + 1`
        // accounting is what makes this work.
        let tarball = make_test_tarball("hi.txt", b"hi");
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&tarball[..])
            .read_to_end(&mut decoded)
            .unwrap();
        let exact = decoded.len() as u64;

        let result = validate_and_unpack(&tarball, &hash, tmp.path(), exact);
        assert!(
            result.is_ok(),
            "cap == archive size must succeed, got {result:?}"
        );
    }

    #[test]
    fn rejects_archive_clearly_over_cap() {
        // Counterpart to the at-cap test: an archive whose uncompressed
        // size is well above the cap must fail with `CapExceeded`. The
        // tar crate reads in 512-byte blocks, so the exact "one byte
        // over" threshold isn't deterministic; a cap clearly smaller
        // than one block (less than 512 bytes) reliably trips
        // `CapExceeded`.
        let tarball = make_test_tarball("hi.txt", b"hi");
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        // 100-byte cap; a single tar header is already 512 bytes.
        let result = validate_and_unpack(&tarball, &hash, tmp.path(), 100);
        assert!(
            matches!(result, Err(ComponentError::CapExceeded(_))),
            "expected CapExceeded for clearly-insufficient cap, got {result:?}"
        );
    }

    #[test]
    fn accepts_archive_at_modest_cap_below_default() {
        // Sanity test: an archive that fits comfortably under a small
        // cap unpacks normally.
        let tarball = make_test_tarball("test.txt", b"hi");
        let hash = sha256_hex(&tarball);
        let tmp = tempfile::tempdir().unwrap();

        let result = validate_and_unpack(&tarball, &hash, tmp.path(), 64 * 1024);
        assert!(
            result.is_ok(),
            "modest archive must fit under 64 KiB cap, got {result:?}"
        );
    }

    // ---------------------------------------------------------------
    // D5: phase 2 (real-path) helper unit-test for TOCTOU defense
    // ---------------------------------------------------------------

    #[test]
    fn phase2_rejects_post_unpack_symlink_swap() {
        // Simulate the TOCTOU attack: after the directory is created
        // by an earlier (benign) entry, a non-tar agent swaps it for
        // a symlink to outside the root. When a subsequent entry's
        // pre-write phase-2 walk canonicalizes the parent, it must
        // detect the escape.
        let tmp = tempfile::tempdir().unwrap();
        let unpack_root = tmp.path().join("inside");
        std::fs::create_dir_all(&unpack_root).unwrap();
        let unpack_root_real = unpack_root.canonicalize().unwrap();

        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        // Place a directory inside the root, then atomically swap it
        // for a symlink to `outside`.
        let safe_dir = unpack_root.join("safe_dir");
        std::fs::create_dir_all(&safe_dir).unwrap();
        std::fs::remove_dir(&safe_dir).unwrap();
        unix_symlink(&outside, &safe_dir).unwrap();

        // Pretend we are about to write `unpack_root/safe_dir/file`.
        let entry_dest = unpack_root_real.join("safe_dir/file");
        let res = verify_parent_realpath_under(&unpack_root_real, &entry_dest);
        assert!(
            res.is_err(),
            "phase 2 must detect the symlink-swap escape, got {res:?}"
        );
    }

    #[test]
    fn phase2_rejects_directory_creation_through_symlink_swap() {
        // Companion to phase2_rejects_post_unpack_symlink_swap: the
        // Directory arm of unpack_one MUST also invoke the phase-2
        // walk. Without it, a `safe_dir/inner_dir` directory entry
        // would land outside the unpack root if a prior symlink swap
        // redirected `safe_dir`. The setup mirrors the file-swap
        // variant; the assertion proves the helper rejects the same
        // way for a directory destination.
        let tmp = tempfile::tempdir().unwrap();
        let unpack_root = tmp.path().join("inside");
        std::fs::create_dir_all(&unpack_root).unwrap();
        let unpack_root_real = unpack_root.canonicalize().unwrap();

        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        let safe_dir = unpack_root.join("safe_dir");
        std::fs::create_dir_all(&safe_dir).unwrap();
        std::fs::remove_dir(&safe_dir).unwrap();
        unix_symlink(&outside, &safe_dir).unwrap();

        // Pretend we are about to create directory
        // `unpack_root/safe_dir/inner_dir`. The phase-2 walk is run
        // against the parent (`safe_dir`), which now canonicalizes
        // outside the unpack root.
        let entry_dest = unpack_root_real.join("safe_dir/inner_dir");
        let res = verify_parent_realpath_under(&unpack_root_real, &entry_dest);
        assert!(
            res.is_err(),
            "phase 2 must detect the symlink-swap escape for a directory \
             destination, got {res:?}"
        );
    }

    #[test]
    fn phase2_accepts_internal_symlink_chain() {
        // Counterpart to the attack test: a symlink that resolves
        // back inside the root must NOT be rejected by phase 2.
        let tmp = tempfile::tempdir().unwrap();
        let unpack_root = tmp.path().join("inside");
        std::fs::create_dir_all(&unpack_root).unwrap();
        let unpack_root_real = unpack_root.canonicalize().unwrap();

        std::fs::create_dir_all(unpack_root.join("real_dir")).unwrap();
        unix_symlink("real_dir", unpack_root.join("alias")).unwrap();

        let entry_dest = unpack_root_real.join("alias/file");
        let res = verify_parent_realpath_under(&unpack_root_real, &entry_dest);
        assert!(res.is_ok(), "internal symlink chain must pass, got {res:?}");
    }

    // ---------------------------------------------------------------
    // lexical_clean unit tests
    // ---------------------------------------------------------------

    #[test]
    fn lexical_clean_handles_basic_cases() {
        assert_eq!(
            lexical_clean(Path::new("/tmp/a/b")).unwrap(),
            PathBuf::from("/tmp/a/b")
        );
        assert_eq!(
            lexical_clean(Path::new("/tmp/a/./b/")).unwrap(),
            PathBuf::from("/tmp/a/b")
        );
        assert_eq!(
            lexical_clean(Path::new("/tmp/a/b/..")).unwrap(),
            PathBuf::from("/tmp/a")
        );
        assert_eq!(
            lexical_clean(Path::new("/tmp/a/b/../../c")).unwrap(),
            PathBuf::from("/tmp/c")
        );
    }

    #[test]
    fn lexical_clean_rejects_escapes() {
        // `lexical_clean` itself only enforces "no `..` past the root
        // marker". It returns `None` for these because `..` pops past
        // an empty stack (relative) or past `/` (absolute).
        assert!(lexical_clean(Path::new("..")).is_none());
        assert!(lexical_clean(Path::new("a/../..")).is_none());
        assert!(lexical_clean(Path::new("/..")).is_none());
        // `/tmp/a/../../etc` cleans to `/etc` — escape from a parent
        // directory is detected by `logical_normalize_within`, not by
        // `lexical_clean` alone.
        assert_eq!(
            lexical_clean(Path::new("/tmp/a/../../etc")).unwrap(),
            PathBuf::from("/etc")
        );
        // But the containment-aware wrapper rejects it.
        assert!(
            logical_normalize_within(Path::new("/tmp/unpack"), Path::new("a/../../etc")).is_none()
        );
    }
}
