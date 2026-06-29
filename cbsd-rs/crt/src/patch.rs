// crt — imported-patch introspection (`patch list` / `patch info`).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only introspection of the content-addressed patch store (design/plan
//! 002): `crt patch list` and `crt patch info`. The functions return data; the
//! CLI renders it either as text (the helpers here) or as JSON
//! (`serde_json::to_string_pretty` in `main`).

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use crt_core::{PatchMeta, Provenance, Sha256};
use crt_store::Store;
use serde::Serialize;

/// All imported patches, sorted by `(subject, blob_hash)` for stable output.
///
/// `Sha256` has no `Ord`, so the hash is ordered as its hex string. Reads one
/// `get_meta` per patch (1 list + N gets); a failing read aborts the listing
/// rather than silently dropping the patch — fail-loud, matching the rest of
/// the tool.
pub async fn list(store: &dyn Store) -> Result<Vec<PatchMeta>> {
    let hashes = store.list_patches().await?;
    let mut patches = Vec::with_capacity(hashes.len());
    for hash in hashes {
        patches.push(store.get_meta(&hash).await?);
    }
    patches.sort_by(|a, b| {
        (&a.subject, a.blob_hash.to_hex()).cmp(&(&b.subject, b.blob_hash.to_hex()))
    });
    Ok(patches)
}

/// Render the patch list as `<blob_hash>  <subject>` lines (one per patch).
#[must_use]
pub fn render_list(patches: &[PatchMeta]) -> String {
    let mut out = String::new();
    for p in patches {
        out.push_str(&format!("{}  {}\n", p.blob_hash, p.subject));
    }
    out
}

/// How `crt patch list --group-by` buckets patches (design §6). Extended with
/// annotation-derived keys (ceph-version, tag) once annotations exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum GroupBy {
    /// One group per upstream PR URL set; local-range imports bucket together
    /// by their range description.
    Pr,
    /// One group per source repository (`source_repo`).
    SourceRepo,
}

/// A named bucket of patches produced by [`group`]. Serializes to
/// `{ "group": <string>, "patches": [<PatchMeta>, …] }` (design §6); the
/// per-element annotations wrapper lands in a later commit.
#[derive(Serialize)]
pub struct PatchGroup<'a> {
    pub group: String,
    pub patches: Vec<&'a PatchMeta>,
}

/// Bucket `patches` for `--group-by`. Groups are ordered by key; within a
/// group, patches keep the seq-002 `(subject, blob_hash)` order.
#[must_use]
pub fn group(patches: &[PatchMeta], by: GroupBy) -> Vec<PatchGroup<'_>> {
    let mut buckets: BTreeMap<String, Vec<&PatchMeta>> = BTreeMap::new();
    for p in patches {
        buckets.entry(group_key(p, by)).or_default().push(p);
    }
    buckets
        .into_iter()
        .map(|(group, mut patches)| {
            patches.sort_by(|a, b| {
                (&a.subject, a.blob_hash.to_hex()).cmp(&(&b.subject, b.blob_hash.to_hex()))
            });
            PatchGroup { group, patches }
        })
        .collect()
}

/// The bucket key for one patch under `by`.
fn group_key(p: &PatchMeta, by: GroupBy) -> String {
    match by {
        GroupBy::Pr => match &p.provenance {
            Provenance::UpstreamPr { prs, state, .. } => {
                // `UpstreamPrState` renders in its serde kebab-case spelling.
                let state = serde_json::to_string(state).unwrap_or_default();
                let state = state.trim_matches('"');
                if prs.is_empty() {
                    format!("(pr) [{state}]")
                } else {
                    format!("{} [{state}]", prs.join(", "))
                }
            }
            Provenance::Other { description } => format!("(local range) {description}"),
        },
        GroupBy::SourceRepo => p.source_repo.clone(),
    }
}

/// Render grouped patches: a header line per group, then indented
/// `<blob_hash>  <subject>` lines. The per-group count summary is the caller's
/// (it goes to stderr).
#[must_use]
pub fn render_groups(groups: &[PatchGroup<'_>]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for g in groups {
        let _ = writeln!(out, "{}", g.group);
        for p in &g.patches {
            let _ = writeln!(out, "  {}  {}", p.blob_hash, p.subject);
        }
    }
    out
}

/// Resolve a patch by `arg` — a full 64-char hex `blob_hash`, or a unique short
/// prefix (4–63 hex) of one — to its [`PatchMeta`].
///
/// `arg` is validated as lowercase hex first; the length must be `4..=64` (`<4`
/// is too short, `>64` is too long). A full hash is looked up directly; a prefix
/// is matched against `list_patches` — no match or an ambiguous prefix is an
/// error.
pub async fn find(store: &dyn Store, arg: &str) -> Result<PatchMeta> {
    if !arg.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        bail!("{arg:?} is not a lowercase-hex blob hash or prefix");
    }
    match arg.len() {
        64 => {
            let hash = Sha256::try_from(arg.to_owned())?;
            store.get_meta(&hash).await.map_err(|e| {
                if e.is_not_found() {
                    anyhow!("no patch with blob hash {hash}")
                } else {
                    e.into()
                }
            })
        }
        4..=63 => {
            let matches: Vec<Sha256> = store
                .list_patches()
                .await?
                .into_iter()
                .filter(|h| h.to_hex().starts_with(arg))
                .collect();
            match matches.as_slice() {
                [] => bail!("no patch with blob hash prefix {arg:?}"),
                [hash] => Ok(store.get_meta(hash).await?),
                many => {
                    let listed = many
                        .iter()
                        .map(Sha256::to_hex)
                        .collect::<Vec<_>>()
                        .join(", ");
                    bail!("blob hash prefix {arg:?} is ambiguous; matches: {listed}")
                }
            }
        }
        n => bail!(
            "{arg:?} has {n} hex chars; expected a 64-char blob hash or a prefix \
             of 4–63"
        ),
    }
}

/// Render a patch's metadata as a labeled text block. When `equivalent` is set
/// it names a *different* blob that is the patch-id index's representative for
/// this patch; the line is text-only (one-way — absent when inspecting that
/// representative — and never part of the `--json` output).
#[must_use]
pub fn render_info(meta: &PatchMeta, equivalent: Option<&Sha256>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "blob_hash:    {}", meta.blob_hash);
    let _ = writeln!(out, "patch_id:     {}", meta.patch_id);
    let _ = writeln!(out, "subject:      {}", meta.subject);
    let _ = writeln!(
        out,
        "author:       {} <{}>",
        meta.author.name, meta.author.email
    );
    let _ = writeln!(out, "authored:     {}", meta.authored);
    let _ = writeln!(out, "source_repo:  {}", meta.source_repo);
    match &meta.provenance {
        Provenance::UpstreamPr {
            prs,
            commits,
            state,
        } => {
            // `UpstreamPrState` is serde kebab-case; render that spelling.
            let state = serde_json::to_string(state).unwrap_or_default();
            let _ = writeln!(
                out,
                "provenance:   upstream-pr [{}]",
                state.trim_matches('"')
            );
            if !prs.is_empty() {
                let _ = writeln!(out, "                prs:     {}", prs.join(", "));
            }
            if !commits.is_empty() {
                let _ = writeln!(out, "                commits: {}", commits.join(", "));
            }
        }
        Provenance::Other { description } => {
            let _ = writeln!(out, "provenance:   other: {description}");
        }
    }
    if !meta.cherry_picked_from.is_empty() {
        let _ = writeln!(
            out,
            "cherry-picked-from: {}",
            meta.cherry_picked_from.join(", ")
        );
    }
    if let Some(other) = equivalent {
        let _ = writeln!(
            out,
            "equivalent-to: {other} (the patch-id index representative)"
        );
    }
    let _ = writeln!(out);
    out.push_str(&meta.body);
    if !meta.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt_core::{Identity, UpstreamPrState, blob_hash};
    use crt_store::ObjectBackedStore;

    /// Build a `PatchMeta` with a chosen provenance and source repo (grouping
    /// is a pure function of those, so the tests need no store).
    fn meta_with(
        raw: &[u8],
        subject: &str,
        provenance: Provenance,
        source_repo: &str,
    ) -> PatchMeta {
        PatchMeta {
            blob_hash: blob_hash(raw),
            patch_id: format!("pid-{subject}"),
            author: Identity {
                name: "n".to_owned(),
                email: "e@example.com".to_owned(),
            },
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: subject.to_owned(),
            body: "b".to_owned(),
            cherry_picked_from: vec![],
            provenance,
            source_repo: source_repo.to_owned(),
        }
    }

    fn pr_provenance(url: &str, state: UpstreamPrState) -> Provenance {
        Provenance::UpstreamPr {
            prs: vec![url.to_owned()],
            commits: vec!["abcdef".to_owned()],
            state,
        }
    }

    #[test]
    fn group_by_pr_buckets_same_pr_together_and_locals_apart() {
        let pr = pr_provenance(
            "https://github.com/ceph/ceph/pull/1",
            UpstreamPrState::MergedMain,
        );
        let local = Provenance::Other {
            description: "ceph 1..2".to_owned(),
        };
        let patches = vec![
            meta_with(b"a", "aaa", pr.clone(), "ceph/ceph"),
            meta_with(b"b", "bbb", pr.clone(), "ceph/ceph"),
            meta_with(b"c", "ccc", local, "/tmp/ceph"),
        ];
        let groups = group(&patches, GroupBy::Pr);
        assert_eq!(groups.len(), 2);
        let pr_group = groups
            .iter()
            .find(|g| g.group.contains("pull/1"))
            .expect("a PR group");
        assert_eq!(pr_group.patches.len(), 2);
        // The PR group header carries the upstream state.
        assert!(
            pr_group.group.contains("merged-main"),
            "got: {}",
            pr_group.group
        );
        let local_group = groups
            .iter()
            .find(|g| g.group.contains("local range"))
            .expect("a local-range group");
        assert_eq!(local_group.patches.len(), 1);
    }

    #[test]
    fn group_by_source_repo_buckets_by_repo_in_key_order() {
        let p = Provenance::Other {
            description: "d".to_owned(),
        };
        let patches = vec![
            meta_with(b"a", "a", p.clone(), "ceph/ceph"),
            meta_with(b"b", "b", p.clone(), "clyso/ceph"),
            meta_with(b"c", "c", p, "ceph/ceph"),
        ];
        let groups = group(&patches, GroupBy::SourceRepo);
        // Groups are ordered by key; "ceph/ceph" sorts before "clyso/ceph".
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "ceph/ceph");
        assert_eq!(groups[0].patches.len(), 2);
        assert_eq!(groups[1].group, "clyso/ceph");
        assert_eq!(groups[1].patches.len(), 1);
    }

    #[test]
    fn render_groups_is_header_then_indented_patches() {
        let p = Provenance::Other {
            description: "d".to_owned(),
        };
        let patches = vec![meta_with(b"a", "the subject", p, "ceph/ceph")];
        let groups = group(&patches, GroupBy::SourceRepo);
        let out = render_groups(&groups);
        assert!(out.starts_with("ceph/ceph\n"), "header first: {out:?}");
        assert!(out.contains("\n  "), "patches are indented: {out:?}");
        assert!(out.contains("the subject"));
    }

    #[test]
    fn grouped_json_has_group_and_patches_fields() {
        let p = Provenance::Other {
            description: "d".to_owned(),
        };
        let patches = vec![meta_with(b"a", "s", p, "ceph/ceph")];
        let groups = group(&patches, GroupBy::SourceRepo);
        let json = serde_json::to_value(&groups).unwrap();
        assert!(json.is_array());
        assert_eq!(json[0]["group"], "ceph/ceph");
        assert!(json[0]["patches"].is_array());
        // Commit 1's grouped element is the bare PatchMeta (the annotations
        // wrapper lands in commit 2c).
        assert_eq!(json[0]["patches"][0]["subject"], "s");
    }

    /// Store a patch with the given subject under content address of `raw`.
    async fn put_patch(store: &dyn Store, raw: &[u8], subject: &str) -> Sha256 {
        let hash = blob_hash(raw);
        let meta = PatchMeta {
            blob_hash: hash,
            patch_id: format!("pid-{subject}"),
            author: Identity {
                name: "n".to_owned(),
                email: "e@example.com".to_owned(),
            },
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: subject.to_owned(),
            body: "b".to_owned(),
            cherry_picked_from: vec![],
            provenance: Provenance::Other {
                description: "d".to_owned(),
            },
            source_repo: "r".to_owned(),
        };
        store.put_blob(&hash, raw).await.unwrap();
        store.put_meta(&hash, &meta).await.unwrap();
        hash
    }

    #[tokio::test]
    async fn list_is_empty_on_an_empty_store() {
        let store = ObjectBackedStore::in_memory();
        assert!(list(&store).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_sorts_by_subject_then_hash() {
        let store = ObjectBackedStore::in_memory();
        put_patch(&store, b"one", "zzz last").await;
        put_patch(&store, b"two", "aaa first").await;
        let patches = list(&store).await.unwrap();
        let subjects: Vec<&str> = patches.iter().map(|p| p.subject.as_str()).collect();
        assert_eq!(subjects, ["aaa first", "zzz last"]);
    }

    #[tokio::test]
    async fn list_json_round_trips() {
        let store = ObjectBackedStore::in_memory();
        put_patch(&store, b"one", "s1").await;
        put_patch(&store, b"two", "s2").await;
        let patches = list(&store).await.unwrap();
        let json = serde_json::to_string_pretty(&patches).unwrap();
        let parsed: Vec<PatchMeta> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, patches);
    }

    #[test]
    fn render_list_is_one_line_per_patch() {
        let store_hash = blob_hash(b"x");
        let meta = PatchMeta {
            blob_hash: store_hash,
            patch_id: "p".to_owned(),
            author: Identity {
                name: "n".to_owned(),
                email: "e@example.com".to_owned(),
            },
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: "the subject".to_owned(),
            body: "b".to_owned(),
            cherry_picked_from: vec![],
            provenance: Provenance::Other {
                description: "d".to_owned(),
            },
            source_repo: "r".to_owned(),
        };
        let rendered = render_list(std::slice::from_ref(&meta));
        assert_eq!(rendered, format!("{}  the subject\n", store_hash));
    }

    #[tokio::test]
    async fn find_resolves_a_full_hash() {
        let store = ObjectBackedStore::in_memory();
        let h = put_patch(&store, b"the patch", "a subject").await;
        let meta = find(&store, &h.to_hex()).await.unwrap();
        assert_eq!(meta.blob_hash, h);
        assert_eq!(meta.subject, "a subject");
    }

    #[tokio::test]
    async fn find_resolves_a_unique_prefix() {
        let store = ObjectBackedStore::in_memory();
        let h = put_patch(&store, b"the patch", "s").await;
        assert_eq!(find(&store, &h.to_hex()[..12]).await.unwrap().blob_hash, h);
    }

    #[tokio::test]
    async fn find_full_hash_not_found_errors() {
        let store = ObjectBackedStore::in_memory();
        let absent = blob_hash(b"never stored").to_hex();
        let err = find(&store, &absent).await.unwrap_err().to_string();
        assert!(err.contains("no patch with blob hash"), "got: {err}");
    }

    #[tokio::test]
    async fn find_no_match_prefix_errors() {
        // Empty store: any valid prefix matches nothing.
        let store = ObjectBackedStore::in_memory();
        let err = find(&store, "abcd").await.unwrap_err().to_string();
        assert!(err.contains("no patch with blob hash prefix"), "got: {err}");
    }

    #[tokio::test]
    async fn find_ambiguous_prefix_errors() {
        use std::collections::HashMap;
        let store = ObjectBackedStore::in_memory();
        // Two distinct inputs whose blob hashes collide on a 4-hex prefix
        // (birthday-cheap over 65536 buckets).
        let mut by_prefix: HashMap<String, Vec<u8>> = HashMap::new();
        let mut collision: Option<(Vec<u8>, Vec<u8>, String)> = None;
        for i in 0u32..200_000 {
            let raw = i.to_le_bytes().to_vec();
            let prefix = blob_hash(&raw).to_hex()[..4].to_owned();
            if let Some(prev) = by_prefix.insert(prefix.clone(), raw.clone()) {
                collision = Some((prev, raw, prefix));
                break;
            }
        }
        let (a, b, prefix) = collision.expect("a 4-hex prefix collision within 200k tries");
        put_patch(&store, &a, "subject a").await;
        put_patch(&store, &b, "subject b").await;
        let err = find(&store, &prefix).await.unwrap_err().to_string();
        assert!(err.contains("ambiguous"), "got: {err}");
    }

    #[tokio::test]
    async fn find_rejects_malformed_args() {
        let store = ObjectBackedStore::in_memory();
        let too_long = "a".repeat(65);
        // uppercase, non-hex, too short, too long.
        for bad in ["ABCD1234", "zzzz", "abc", too_long.as_str()] {
            assert!(
                find(&store, bad).await.is_err(),
                "expected error for {bad:?}"
            );
        }
    }

    #[test]
    fn render_info_contains_the_key_fields() {
        let h = blob_hash(b"x");
        let meta = PatchMeta {
            blob_hash: h,
            patch_id: "the-patch-id".to_owned(),
            author: Identity {
                name: "Ann".to_owned(),
                email: "ann@example.com".to_owned(),
            },
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: "fix the thing".to_owned(),
            body: "the body".to_owned(),
            cherry_picked_from: vec![],
            provenance: Provenance::Other {
                description: "from somewhere".to_owned(),
            },
            source_repo: "ceph/ceph".to_owned(),
        };
        let out = render_info(&meta, None);
        assert!(out.contains("the-patch-id"));
        assert!(out.contains("fix the thing"));
        assert!(out.contains("Ann <ann@example.com>"));
        assert!(out.contains("the body"));
        assert!(!out.contains("equivalent-to"));
    }

    #[test]
    fn render_info_shows_equivalence_when_set() {
        let h = blob_hash(b"x");
        let other = blob_hash(b"y");
        let meta = PatchMeta {
            blob_hash: h,
            patch_id: "p".to_owned(),
            author: Identity {
                name: "n".to_owned(),
                email: "e@example.com".to_owned(),
            },
            authored: "2026-06-21T00:00:00+00:00".to_owned(),
            subject: "s".to_owned(),
            body: "b".to_owned(),
            cherry_picked_from: vec![],
            provenance: Provenance::Other {
                description: "d".to_owned(),
            },
            source_repo: "r".to_owned(),
        };
        let out = render_info(&meta, Some(&other));
        assert!(out.contains("equivalent-to"));
        assert!(out.contains(&other.to_hex()));
    }
}
