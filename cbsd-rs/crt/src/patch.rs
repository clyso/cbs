// crt — imported-patch introspection (`patch list` / `patch info`).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only introspection of the content-addressed patch store (design/plan
//! 002): `crt patch list` and `crt patch info`. The functions return data; the
//! CLI renders it either as text (the helpers here) or as JSON
//! (`serde_json::to_string_pretty` in `main`).

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use crt_core::{
    Applicability, PatchAnnotations, PatchMeta, Provenance, Sha256, VersionQuery, VersionSpec,
    parse_version_query,
};
use crt_store::Store;
use serde::Serialize;

/// A `patch list` filter (design §6). Empty fields are inert; set fields
/// compose with **and** semantics.
pub struct Filter {
    /// Keep patches applicable to this ceph version (`Generic` ∪ matching).
    pub ceph_version: Option<VersionQuery>,
    /// Keep patches whose tags contain this tag.
    pub tag: Option<String>,
    /// Keep patches with `applies_to = None` (unassessed; includes patches
    /// with no annotations record at all).
    pub unassessed: bool,
}

impl Filter {
    /// Build from `list` flags, parsing `--ceph-version` into a query (§7).
    pub fn from_flags(
        ceph_version: Option<&str>,
        tag: Option<String>,
        unassessed: bool,
    ) -> Result<Self> {
        let ceph_version = ceph_version.map(parse_version_query).transpose()?;
        Ok(Self {
            ceph_version,
            tag,
            unassessed,
        })
    }
}

/// Whether `view` passes `filter` (design §6). A patch with no annotations
/// record reads as unassessed: it matches `--unassessed` but neither the
/// version nor the tag filter (§7 — `None` is never assumed generic).
#[must_use]
pub fn matches_filter(view: &PatchView, filter: &Filter) -> bool {
    let applies = view
        .annotations
        .as_ref()
        .and_then(|a| a.applies_to.as_ref());
    if let Some(q) = &filter.ceph_version
        && !applies.is_some_and(|a| a.matches(q))
    {
        return false;
    }
    if let Some(tag) = &filter.tag
        && !view
            .annotations
            .as_ref()
            .is_some_and(|a| a.tags.contains(tag))
    {
        return false;
    }
    if filter.unassessed && applies.is_some() {
        return false;
    }
    true
}

/// A patch's git-derived metadata plus its operator-authored annotations, if
/// any (design §6). The element of `patch list --json` and the `patch info`
/// JSON: `{ "meta": <PatchMeta>, "annotations": <PatchAnnotations> | null }`
/// (explicit `null` when the blob has no annotations record).
#[derive(Debug, Serialize)]
pub struct PatchView {
    pub meta: PatchMeta,
    pub annotations: Option<PatchAnnotations>,
}

/// All imported patches, sorted by `(subject, blob_hash)` for stable output.
///
/// `Sha256` has no `Ord`, so the hash is ordered as its hex string. Reads one
/// `get_meta` + one `get_annotations` per patch (1 list + 2N gets); a failing
/// read aborts the listing rather than silently dropping the patch — fail-loud,
/// matching the rest of the tool.
pub async fn list(store: &dyn Store) -> Result<Vec<PatchView>> {
    let hashes = store.list_patches().await?;
    let mut patches = Vec::with_capacity(hashes.len());
    for hash in hashes {
        let meta = store.get_meta(&hash).await?;
        let annotations = store.get_annotations(&hash).await?;
        patches.push(PatchView { meta, annotations });
    }
    patches.sort_by(|a, b| {
        (&a.meta.subject, a.meta.blob_hash.to_hex())
            .cmp(&(&b.meta.subject, b.meta.blob_hash.to_hex()))
    });
    Ok(patches)
}

/// A one-line annotations summary for the list column, e.g. `[18.2 | rgw]`
/// (design §6) — `None` when the blob has no (meaningful) annotations record,
/// so un-annotated patches render as a bare `<hash>  <subject>` line.
fn annotation_summary(view: &PatchView) -> Option<String> {
    let ann = view.annotations.as_ref()?;
    if ann.is_empty() {
        return None;
    }
    let applies = match &ann.applies_to {
        None => "unassessed".to_owned(),
        Some(Applicability::Generic) => "generic".to_owned(),
        Some(Applicability::Versions(specs)) => {
            specs.iter().map(render_spec).collect::<Vec<_>>().join(",")
        }
    };
    if ann.tags.is_empty() {
        Some(format!("[{applies}]"))
    } else {
        let tags = ann.tags.iter().cloned().collect::<Vec<_>>().join(",");
        Some(format!("[{applies} | {tags}]"))
    }
}

/// One list line: `<blob_hash>  <subject>` plus, when present, the annotations
/// summary column.
fn patch_line(view: &PatchView) -> String {
    match annotation_summary(view) {
        Some(summary) => format!("{}  {}  {summary}", view.meta.blob_hash, view.meta.subject),
        None => format!("{}  {}", view.meta.blob_hash, view.meta.subject),
    }
}

/// Render the patch list, one [`patch_line`] per patch.
#[must_use]
pub fn render_list(patches: &[PatchView]) -> String {
    let mut out = String::new();
    for p in patches {
        out.push_str(&patch_line(p));
        out.push('\n');
    }
    out
}

/// How `crt patch list --group-by` buckets patches (design §6). The
/// annotation-derived keys (`CephVersion`, `Tag`) put a patch in **every**
/// matching group (multi-membership).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum GroupBy {
    /// One group per upstream PR URL set; local-range imports bucket together
    /// by their range description.
    Pr,
    /// One group per source repository (`source_repo`).
    SourceRepo,
    /// One group per applicability spec (`18.2`, `v18.2.0`); `Generic` patches
    /// form a `(generic)` group and unassessed ones an `(unassessed)` group.
    CephVersion,
    /// One group per tag; patches with no tags form an `(untagged)` group.
    Tag,
}

/// A named bucket of patches produced by [`group`]. Serializes to
/// `{ "group": <string>, "patches": [<PatchView>, …] }` (design §6) — each
/// element carrying meta + annotations.
#[derive(Serialize)]
pub struct PatchGroup<'a> {
    pub group: String,
    pub patches: Vec<&'a PatchView>,
}

/// Bucket `patches` for `--group-by`. Groups are ordered by key; within a
/// group, patches keep the seq-002 `(subject, blob_hash)` order. A patch may
/// land in several groups under the annotation-derived keys (§6).
#[must_use]
pub fn group(patches: &[PatchView], by: GroupBy) -> Vec<PatchGroup<'_>> {
    let mut buckets: BTreeMap<String, Vec<&PatchView>> = BTreeMap::new();
    for p in patches {
        for key in group_keys(p, by) {
            buckets.entry(key).or_default().push(p);
        }
    }
    buckets
        .into_iter()
        .map(|(group, mut patches)| {
            patches.sort_by(|a, b| {
                (&a.meta.subject, a.meta.blob_hash.to_hex())
                    .cmp(&(&b.meta.subject, b.meta.blob_hash.to_hex()))
            });
            PatchGroup { group, patches }
        })
        .collect()
}

/// The bucket key(s) for one patch under `by`. `Pr`/`SourceRepo` yield exactly
/// one; `CephVersion`/`Tag` yield one per matching value (multi-membership).
fn group_keys(view: &PatchView, by: GroupBy) -> Vec<String> {
    match by {
        GroupBy::Pr => vec![match &view.meta.provenance {
            Provenance::UpstreamPr { prs, state, .. } => {
                // `UpstreamPrState` renders in its serde kebab-case spelling.
                let state = serde_json::to_string(state).unwrap_or_default();
                let state = state.trim_matches('"').to_owned();
                if prs.is_empty() {
                    format!("(pr) [{state}]")
                } else {
                    format!("{} [{state}]", prs.join(", "))
                }
            }
            Provenance::Other { description } => format!("(local range) {description}"),
        }],
        GroupBy::SourceRepo => vec![view.meta.source_repo.clone()],
        GroupBy::CephVersion => {
            match view
                .annotations
                .as_ref()
                .and_then(|a| a.applies_to.as_ref())
            {
                None => vec!["(unassessed)".to_owned()],
                Some(Applicability::Generic) => vec!["(generic)".to_owned()],
                // An empty set never persists (§5), but stay defensive.
                Some(Applicability::Versions(specs)) if specs.is_empty() => {
                    vec!["(unassessed)".to_owned()]
                }
                Some(Applicability::Versions(specs)) => {
                    specs.iter().map(|s| render_spec(s).to_owned()).collect()
                }
            }
        }
        GroupBy::Tag => match view.annotations.as_ref() {
            Some(ann) if !ann.tags.is_empty() => ann.tags.iter().cloned().collect(),
            _ => vec!["(untagged)".to_owned()],
        },
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
            let _ = writeln!(out, "  {}", patch_line(p));
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
pub async fn find(store: &dyn Store, arg: &str) -> Result<PatchView> {
    if !arg.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        bail!("{arg:?} is not a lowercase-hex blob hash or prefix");
    }
    let meta = match arg.len() {
        64 => {
            let hash = Sha256::try_from(arg.to_owned())?;
            store.get_meta(&hash).await.map_err(|e| {
                if e.is_not_found() {
                    anyhow!("no patch with blob hash {hash}")
                } else {
                    e.into()
                }
            })?
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
                [hash] => store.get_meta(hash).await?,
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
    };
    let annotations = store.get_annotations(&meta.blob_hash).await?;
    Ok(PatchView { meta, annotations })
}

/// Render a [`VersionSpec`] for display — the stored string (`18.2` for a
/// `Line`, `v18.2.0` for an `Exact`, design §6).
#[must_use]
pub(crate) fn render_spec(spec: &VersionSpec) -> &str {
    match spec {
        VersionSpec::Line(l) | VersionSpec::Exact(l) => l,
    }
}

/// Append the operator-authored annotations to a `patch info` block. Always
/// shows `applies-to` (an existing record asserts something there, even if
/// `(unassessed)`); the optional facets show only when present.
fn append_annotations(out: &mut String, ann: &PatchAnnotations) {
    use std::fmt::Write as _;
    match &ann.applies_to {
        None => {
            let _ = writeln!(out, "applies-to:   (unassessed)");
        }
        Some(Applicability::Generic) => {
            let _ = writeln!(out, "applies-to:   generic");
        }
        Some(Applicability::Versions(specs)) => {
            let list = specs.iter().map(render_spec).collect::<Vec<_>>().join(", ");
            let _ = writeln!(out, "applies-to:   versions: {list}");
        }
    }
    if !ann.tags.is_empty() {
        let tags = ann.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        let _ = writeln!(out, "tags:         {tags}");
    }
    if let Some(desc) = &ann.description {
        let _ = writeln!(out, "description:  {desc}");
    }
    if !ann.attributes.is_empty() {
        let kv = ann
            .attributes
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "attributes:   {kv}");
    }
}

/// Render a patch's metadata as a labeled text block. `annotations` is the
/// blob's annotations record, if any (design §6). When `equivalent` is set it
/// names a *different* blob that is the patch-id index's representative for
/// this patch; the line is text-only (one-way — absent when inspecting that
/// representative — and never part of the `--json` output).
#[must_use]
pub fn render_info(
    meta: &PatchMeta,
    annotations: Option<&PatchAnnotations>,
    equivalent: Option<&Sha256>,
) -> String {
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
    if let Some(ann) = annotations
        && !ann.is_empty()
    {
        append_annotations(&mut out, ann);
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

    /// A `PatchView` with no annotations — grouping ignores annotations, so
    /// the grouping tests build these directly without a store.
    fn view_with(
        raw: &[u8],
        subject: &str,
        provenance: Provenance,
        source_repo: &str,
    ) -> PatchView {
        PatchView {
            meta: meta_with(raw, subject, provenance, source_repo),
            annotations: None,
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
            view_with(b"a", "aaa", pr.clone(), "ceph/ceph"),
            view_with(b"b", "bbb", pr.clone(), "ceph/ceph"),
            view_with(b"c", "ccc", local, "/tmp/ceph"),
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
            view_with(b"a", "a", p.clone(), "ceph/ceph"),
            view_with(b"b", "b", p.clone(), "clyso/ceph"),
            view_with(b"c", "c", p, "ceph/ceph"),
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
        let patches = vec![view_with(b"a", "the subject", p, "ceph/ceph")];
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
        let patches = vec![view_with(b"a", "s", p, "ceph/ceph")];
        let groups = group(&patches, GroupBy::SourceRepo);
        let json = serde_json::to_value(&groups).unwrap();
        assert!(json.is_array());
        assert_eq!(json[0]["group"], "ceph/ceph");
        assert!(json[0]["patches"].is_array());
        // Each grouped element is a `{meta, annotations}` view (design §6).
        assert_eq!(json[0]["patches"][0]["meta"]["subject"], "s");
        assert!(json[0]["patches"][0]["annotations"].is_null());
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
        let subjects: Vec<&str> = patches.iter().map(|p| p.meta.subject.as_str()).collect();
        assert_eq!(subjects, ["aaa first", "zzz last"]);
    }

    #[tokio::test]
    async fn list_json_emits_meta_and_null_annotations_elements() {
        let store = ObjectBackedStore::in_memory();
        put_patch(&store, b"one", "s1").await;
        put_patch(&store, b"two", "s2").await;
        let patches = list(&store).await.unwrap();
        let json = serde_json::to_value(&patches).unwrap();
        // Flat `--json` is now an array of `{meta, annotations}` elements; a
        // patch with no annotations record renders explicit `null` (design §6).
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["meta"]["subject"], "s1");
        assert!(json[0]["annotations"].is_null());
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
        let view = PatchView {
            meta,
            annotations: None,
        };
        let rendered = render_list(std::slice::from_ref(&view));
        assert_eq!(rendered, format!("{}  the subject\n", store_hash));
    }

    #[tokio::test]
    async fn find_resolves_a_full_hash() {
        let store = ObjectBackedStore::in_memory();
        let h = put_patch(&store, b"the patch", "a subject").await;
        let view = find(&store, &h.to_hex()).await.unwrap();
        assert_eq!(view.meta.blob_hash, h);
        assert_eq!(view.meta.subject, "a subject");
    }

    #[tokio::test]
    async fn find_resolves_a_unique_prefix() {
        let store = ObjectBackedStore::in_memory();
        let h = put_patch(&store, b"the patch", "s").await;
        assert_eq!(
            find(&store, &h.to_hex()[..12])
                .await
                .unwrap()
                .meta
                .blob_hash,
            h
        );
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
        let out = render_info(&meta, None, None);
        assert!(out.contains("the-patch-id"));
        assert!(out.contains("fix the thing"));
        assert!(out.contains("Ann <ann@example.com>"));
        assert!(out.contains("the body"));
        assert!(!out.contains("equivalent-to"));
        // No annotations record ⇒ no applies-to line.
        assert!(!out.contains("applies-to"));
    }

    #[test]
    fn render_info_shows_annotations_when_present() {
        let meta = meta_with(
            b"x",
            "s",
            Provenance::Other {
                description: "d".to_owned(),
            },
            "r",
        );
        let mut ann = PatchAnnotations {
            applies_to: Some(Applicability::Versions(
                [
                    VersionSpec::Line("18.2".to_owned()),
                    VersionSpec::Exact("v18.2.0".to_owned()),
                ]
                .into_iter()
                .collect(),
            )),
            description: Some("the rgw backport".to_owned()),
            ..Default::default()
        };
        ann.tags.insert("rgw".to_owned());
        ann.attributes
            .insert("retire-when".to_owned(), "v18.3.0".to_owned());

        let out = render_info(&meta, Some(&ann), None);
        assert!(out.contains("applies-to:"), "got: {out}");
        assert!(out.contains("18.2"));
        assert!(out.contains("v18.2.0"));
        assert!(out.contains("tags:"));
        assert!(out.contains("rgw"));
        assert!(out.contains("description:"));
        assert!(out.contains("the rgw backport"));
        assert!(out.contains("retire-when=v18.3.0"));
    }

    #[test]
    fn render_info_marks_a_recorded_but_unassessed_patch() {
        let meta = meta_with(
            b"x",
            "s",
            Provenance::Other {
                description: "d".to_owned(),
            },
            "r",
        );
        // A record exists (e.g. only tags set) but applies_to is None.
        let mut ann = PatchAnnotations::default();
        ann.tags.insert("rgw".to_owned());
        let out = render_info(&meta, Some(&ann), None);
        assert!(out.contains("applies-to:   (unassessed)"), "got: {out}");
    }

    #[test]
    fn render_info_hides_an_empty_annotations_record() {
        // An emptied record (e.g. after `--untag <last>`) is non-`None` but
        // `is_empty()`; `patch info` renders no annotations block, matching the
        // bare line `patch list` shows for it.
        let meta = meta_with(
            b"x",
            "s",
            Provenance::Other {
                description: "d".to_owned(),
            },
            "r",
        );
        let empty = PatchAnnotations::default();
        assert!(empty.is_empty());
        let out = render_info(&meta, Some(&empty), None);
        assert!(
            !out.contains("applies-to"),
            "no block for empty record: {out}"
        );
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
        let out = render_info(&meta, None, Some(&other));
        assert!(out.contains("equivalent-to"));
        assert!(out.contains(&other.to_hex()));
    }

    fn versions(specs: &[VersionSpec]) -> Applicability {
        Applicability::Versions(specs.iter().cloned().collect())
    }

    /// A `PatchView` carrying an annotations record (applicability + tags).
    fn view_annotated(
        raw: &[u8],
        subject: &str,
        applies: Option<Applicability>,
        tags: &[&str],
    ) -> PatchView {
        let mut annotations = PatchAnnotations {
            applies_to: applies,
            ..Default::default()
        };
        annotations.tags = tags.iter().map(|t| (*t).to_owned()).collect();
        PatchView {
            meta: meta_with(
                raw,
                subject,
                Provenance::Other {
                    description: "d".to_owned(),
                },
                "r",
            ),
            annotations: Some(annotations),
        }
    }

    #[test]
    fn filter_by_ceph_version_includes_generic_and_matching() {
        let patches = [
            view_annotated(
                b"a",
                "a",
                Some(versions(&[VersionSpec::Line("18.2".to_owned())])),
                &[],
            ),
            view_annotated(b"b", "b", Some(Applicability::Generic), &[]),
            view_annotated(
                b"c",
                "c",
                Some(versions(&[VersionSpec::Line("19.2".to_owned())])),
                &[],
            ),
            view_with(
                b"d",
                "d",
                Provenance::Other {
                    description: "x".to_owned(),
                },
                "r",
            ), // no record
        ];
        let f = Filter::from_flags(Some("v18.2.1"), None, false).unwrap();
        let kept: Vec<&str> = patches
            .iter()
            .filter(|p| matches_filter(p, &f))
            .map(|p| p.meta.subject.as_str())
            .collect();
        // The 18.2 line matches; Generic always matches; 19.2 and the
        // unassessed (no-record) patch do not.
        assert_eq!(kept, ["a", "b"]);
    }

    #[test]
    fn filter_by_tag_and_unassessed() {
        let patches = [
            view_annotated(b"a", "a", Some(Applicability::Generic), &["rgw"]),
            view_annotated(
                b"b",
                "b",
                Some(versions(&[VersionSpec::Line("18.2".to_owned())])),
                &["osd"],
            ),
            view_with(
                b"c",
                "c",
                Provenance::Other {
                    description: "x".to_owned(),
                },
                "r",
            ), // no record
        ];
        let by_tag = Filter::from_flags(None, Some("rgw".to_owned()), false).unwrap();
        let tagged: Vec<&str> = patches
            .iter()
            .filter(|p| matches_filter(p, &by_tag))
            .map(|p| p.meta.subject.as_str())
            .collect();
        assert_eq!(tagged, ["a"]);

        let by_unassessed = Filter::from_flags(None, None, true).unwrap();
        let unassessed: Vec<&str> = patches
            .iter()
            .filter(|p| matches_filter(p, &by_unassessed))
            .map(|p| p.meta.subject.as_str())
            .collect();
        // Only the no-record patch is unassessed (the other two have applies_to).
        assert_eq!(unassessed, ["c"]);
    }

    #[test]
    fn group_by_ceph_version_is_multi_membership() {
        let patches = vec![
            view_annotated(
                b"a",
                "a",
                Some(versions(&[
                    VersionSpec::Line("18.2".to_owned()),
                    VersionSpec::Exact("v19.2.0".to_owned()),
                ])),
                &[],
            ),
            view_annotated(b"b", "b", Some(Applicability::Generic), &[]),
            view_with(
                b"c",
                "c",
                Provenance::Other {
                    description: "x".to_owned(),
                },
                "r",
            ),
        ];
        let groups = group(&patches, GroupBy::CephVersion);
        let keys: Vec<&str> = groups.iter().map(|g| g.group.as_str()).collect();
        // "a" lands under both its specs; Generic and the no-record patch get
        // their own buckets. BTreeMap orders keys.
        assert_eq!(keys, ["(generic)", "(unassessed)", "18.2", "v19.2.0"]);
        let v182 = groups.iter().find(|g| g.group == "18.2").unwrap();
        assert_eq!(v182.patches.len(), 1);
        assert_eq!(v182.patches[0].meta.subject, "a");
    }

    #[test]
    fn group_by_tag_buckets_each_tag_and_untagged() {
        let patches = vec![
            view_annotated(b"a", "a", None, &["rgw", "osd"]),
            view_annotated(b"b", "b", None, &["rgw"]),
            view_with(
                b"c",
                "c",
                Provenance::Other {
                    description: "x".to_owned(),
                },
                "r",
            ),
        ];
        let groups = group(&patches, GroupBy::Tag);
        let keys: Vec<&str> = groups.iter().map(|g| g.group.as_str()).collect();
        assert_eq!(keys, ["(untagged)", "osd", "rgw"]);
        let rgw = groups.iter().find(|g| g.group == "rgw").unwrap();
        assert_eq!(rgw.patches.len(), 2);
    }

    #[test]
    fn render_list_shows_the_annotation_column() {
        let patches = vec![
            view_annotated(
                b"a",
                "annotated",
                Some(versions(&[VersionSpec::Line("18.2".to_owned())])),
                &["rgw"],
            ),
            view_with(
                b"b",
                "bare",
                Provenance::Other {
                    description: "x".to_owned(),
                },
                "r",
            ),
        ];
        let out = render_list(&patches);
        assert!(out.contains("annotated  [18.2 | rgw]"), "got: {out}");
        // The un-annotated patch keeps the bare two-column line.
        let bare_line = out.lines().find(|l| l.contains("bare")).unwrap();
        assert!(
            !bare_line.contains('['),
            "bare line has no column: {bare_line}"
        );
    }
}
