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

//! The external image descriptor and its lookup (design 006). Source:
//! `cbscore/images/desc.py`. `ImageDescriptor` is a repo-authored, **unversioned**
//! input (no `schema_version`).
//!
//! The Python `get_image_desc` is broken — its filename filter is a *raw* string
//! `r"^.*{m[1]}.*.json"` (never interpolated), so no candidate ever matches and
//! it always reports "missing". This implements the *intended* contract: extract
//! `M.m.p` from the version (leading `v` optional), pre-filter `<repo>/desc/`
//! `*.json` by that substring, and authoritatively match exact membership of the
//! version in a descriptor's `releases`. Malformed candidates are skipped; two
//! matches conflict; none is "no such version".

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::versions::parse_version;

/// A source/destination image pair in an image descriptor.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageLocations {
    pub src: String,
    pub dst: String,
}

/// An external, repo-authored image descriptor (unversioned).
#[derive(Debug, Clone, Deserialize)]
pub struct ImageDescriptor {
    pub releases: Vec<String>,
    pub images: Vec<ImageLocations>,
}

/// An error from the image-descriptor lookup.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// No image descriptor claims this version (or `desc/` is absent, or the
    /// version has no `M.m.p` to key on).
    #[error("no image descriptor for version '{0}'")]
    NoSuchVersion(String),
    /// Two descriptors claim the same version.
    #[error("conflicting image descriptors for version '{version}': '{first}' and '{second}'")]
    Conflict {
        version: String,
        first: Utf8PathBuf,
        second: Utf8PathBuf,
    },
}

/// Find the image descriptor whose `releases` contains `version`, searching
/// `<repo_root>/desc/`. Intended only for versions that yield an `M.m.p`; a
/// version without one (a UUIDv7 or patch-less `20.2`) has nothing to key on and
/// yields [`ImageError::NoSuchVersion`] (callers skip the check for such
/// versions — design 006).
pub async fn get_image_desc(
    repo_root: &Utf8Path,
    version: &str,
) -> Result<ImageDescriptor, ImageError> {
    let Some(mmp) = mmp_of(version) else {
        return Err(ImageError::NoSuchVersion(version.to_string()));
    };

    let desc_dir = repo_root.join("desc");
    if !tokio::fs::try_exists(&desc_dir).await.unwrap_or(false) {
        return Err(ImageError::NoSuchVersion(version.to_string()));
    }

    let mut found: Option<(Utf8PathBuf, ImageDescriptor)> = None;
    for candidate in collect_json_candidates(&desc_dir, &mmp).await {
        let Ok(raw) = tokio::fs::read_to_string(&candidate).await else {
            continue;
        };
        // Skip a malformed candidate (matching Python's lenient release listing).
        let Ok(desc) = serde_json::from_str::<ImageDescriptor>(&raw) else {
            continue;
        };
        // Authoritative match: exact membership, in the raw stored form (the
        // `v`-optional extraction above only drives the file pre-filter).
        if desc.releases.iter().any(|r| r == version) {
            if let Some((first, _)) = &found {
                return Err(ImageError::Conflict {
                    version: version.to_string(),
                    first: first.clone(),
                    second: candidate,
                });
            }
            found = Some((candidate, desc));
        }
    }

    found
        .map(|(_, desc)| desc)
        .ok_or_else(|| ImageError::NoSuchVersion(version.to_string()))
}

/// Extract `<M>.<m>.<p>` from a version (leading `v` optional), or `None` when it
/// has no patch component.
fn mmp_of(version: &str) -> Option<String> {
    let parsed = parse_version(version).ok()?;
    match (parsed.minor, parsed.patch) {
        (Some(minor), Some(patch)) => Some(format!("{}.{}.{}", parsed.major, minor, patch)),
        _ => None,
    }
}

/// Recursively collect `*.json` files under `dir` whose filename contains `mmp`
/// (the cheap pre-filter; the membership check below is authoritative).
async fn collect_json_candidates(dir: &Utf8Path, mmp: &str) -> Vec<Utf8PathBuf> {
    let mut candidates = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path);
            } else if path.extension() == Some("json")
                && path.file_name().is_some_and(|n| n.contains(mmp))
            {
                candidates.push(path);
            }
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn write_desc(dir: &Utf8Path, name: &str, json: &str) {
        tokio::fs::create_dir_all(dir.join("desc")).await.unwrap();
        tokio::fs::write(dir.join("desc").join(name), json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resolves_exact_release_membership() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        write_desc(
            root,
            "ceph-20.2.1.json",
            r#"{"releases": ["20.2.1", "20.2.2"], "images": [{"src": "a", "dst": "b"}]}"#,
        )
        .await;

        let desc = get_image_desc(root, "20.2.1").await.unwrap();
        assert_eq!(desc.images[0].dst, "b");
    }

    #[tokio::test]
    async fn missing_dir_or_no_match_is_no_such_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        // No desc/ directory at all.
        assert!(matches!(
            get_image_desc(root, "20.2.1").await,
            Err(ImageError::NoSuchVersion(_))
        ));

        // A descriptor that does not list the version.
        write_desc(
            root,
            "other-20.2.1.json",
            r#"{"releases": ["99.9.9"], "images": []}"#,
        )
        .await;
        assert!(matches!(
            get_image_desc(root, "20.2.1").await,
            Err(ImageError::NoSuchVersion(_))
        ));
    }

    #[tokio::test]
    async fn two_claimants_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        write_desc(
            root,
            "a-20.2.1.json",
            r#"{"releases": ["20.2.1"], "images": []}"#,
        )
        .await;
        write_desc(
            root,
            "b-20.2.1.json",
            r#"{"releases": ["20.2.1"], "images": []}"#,
        )
        .await;
        assert!(matches!(
            get_image_desc(root, "20.2.1").await,
            Err(ImageError::Conflict { .. })
        ));
    }

    #[tokio::test]
    async fn malformed_candidate_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        write_desc(root, "bad-20.2.1.json", "{ not json").await;
        write_desc(
            root,
            "good-20.2.1.json",
            r#"{"releases": ["20.2.1"], "images": []}"#,
        )
        .await;
        // The malformed file is skipped; the good one still resolves.
        assert!(get_image_desc(root, "20.2.1").await.is_ok());
    }

    #[tokio::test]
    async fn version_without_mmp_has_nothing_to_key_on() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(tmp.path()).unwrap();
        // A UUIDv7 and a patch-less version both lack an M.m.p.
        assert!(matches!(
            get_image_desc(root, "019efe1c-3497-7fa1-aa40-abeb95e1be14").await,
            Err(ImageError::NoSuchVersion(_))
        ));
        assert!(matches!(
            get_image_desc(root, "20.2").await,
            Err(ImageError::NoSuchVersion(_))
        ));
    }
}
