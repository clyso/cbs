// CRT core — patch-intrinsic metadata.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Patch-intrinsic, visibility-neutral facts (design §3). One `PatchMeta`
//! per blob, stored at `patches/meta/sha256/<blob_hash>.json`.

use serde::{Deserialize, Serialize};

use crate::Sha256;

/// An author or committer identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

/// Snapshot of an upstream PR's state at import time (design §3 / §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamPrState {
    MergedStable,
    MergedMain,
    ApprovedOpen,
    OpenInReview,
    /// Closed upstream without being merged. The change is still importable
    /// downstream (a patch upstream declined is a valid downstream patch).
    Declined,
}

/// Where a patch came from. `Other` is the downstream-only case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Provenance {
    UpstreamPr {
        /// Upstream PR URLs.
        prs: Vec<String>,
        /// Head commit SHA of each PR (parallel to `prs`).
        commits: Vec<String>,
        state: UpstreamPrState,
    },
    Other {
        /// Free-text origin (e.g. the source repo + range).
        description: String,
    },
}

/// Patch-intrinsic facts, keyed by `blob_hash` in the store (design §3 / §4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchMeta {
    /// Content address of the raw patch blob.
    pub blob_hash: Sha256,
    /// `git patch-id --stable` of the change — offset-invariant identity.
    pub patch_id: String,
    pub author: Identity,
    /// Author date, ISO-8601 (informational).
    pub authored: String,
    pub subject: String,
    pub body: String,
    /// Upstream commits this was cherry-picked from, parsed from the body.
    pub cherry_picked_from: Vec<String>,
    pub provenance: Provenance,
    /// The repository the patch was imported from.
    pub source_repo: String,
}

/// Extract `(cherry picked from commit <sha>)` references from a commit body.
#[must_use]
pub fn cherry_picked_from(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("(cherry picked from commit ")
                .and_then(|rest| rest.strip_suffix(')'))
                .map(|sha| sha.trim().to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cherry_picks_are_parsed() {
        let body = "Fix a thing.\n\n\
                    (cherry picked from commit deadbeef)\n\
                    (cherry picked from commit cafef00d )\n";
        assert_eq!(cherry_picked_from(body), vec!["deadbeef", "cafef00d"]);
        assert!(cherry_picked_from("no picks here").is_empty());
    }

    #[test]
    fn upstream_pr_state_declined_round_trips() {
        // The Declined variant (closed-but-unmerged PRs) must serialize
        // kebab-cased and round-trip — guards against a serde rename drift.
        let json = serde_json::to_string(&UpstreamPrState::Declined).expect("serializes");
        assert_eq!(json, "\"declined\"");
        let back: UpstreamPrState = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, UpstreamPrState::Declined);
    }

    #[test]
    fn patch_meta_round_trips_through_json() {
        let meta = PatchMeta {
            blob_hash: crate::blob_hash(b"patch"),
            patch_id: "abc123".to_owned(),
            author: Identity {
                name: "A".to_owned(),
                email: "a@example.com".to_owned(),
            },
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: "Subject".to_owned(),
            body: "Body".to_owned(),
            cherry_picked_from: vec!["deadbeef".to_owned()],
            provenance: Provenance::Other {
                description: "ceph 1..2".to_owned(),
            },
            source_repo: "/tmp/ceph".to_owned(),
        };
        let json = serde_json::to_string(&meta).expect("serializes");
        let back: PatchMeta = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(meta, back);
    }
}
