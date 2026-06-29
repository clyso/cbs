// crt — applying operator annotations to patches.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Maps annotation CLI flags to the design §5 state transitions and merges
//! them into a blob's annotations record. Holds the pure transition logic
//! (the §9 "no silent applicability claims" invariant lives here, not in the
//! pure `crt-core` types) plus the store read-modify-write. This commit wires
//! up the bulk-at-import path; the per-patch `annotate` command and the extra
//! transitions it needs (remove, unassess, …) land in a later commit.

use anyhow::{Result, bail};
use crt_core::{Applicability, PatchAnnotations, Sha256, VersionSpec, parse_version_spec};
use crt_store::Store;

/// The applicability half of a bulk annotation (design §5.1) — `--ceph-version`
/// and `--generic` are mutually exclusive at the CLI.
pub enum BulkApplies {
    Generic,
    Version(VersionSpec),
}

/// A bulk annotation applied to every patch of an import: an optional
/// applicability assertion plus tags to add. Tag-only is valid.
pub struct BulkAnnotation {
    pub applies: Option<BulkApplies>,
    pub tags: Vec<String>,
}

impl BulkAnnotation {
    /// Build from import flags, returning `None` when none were given — a
    /// flag-less import touches no annotations (§5.1). `--ceph-version` is
    /// parsed/validated up front so a bad version fails before any import work.
    pub fn from_flags(
        ceph_version: Option<&str>,
        generic: bool,
        tags: Vec<String>,
    ) -> Result<Option<Self>> {
        let applies = match (ceph_version, generic) {
            // clap's `conflicts_with` already rejects this; guard defensively.
            (Some(_), true) => bail!("--ceph-version and --generic are mutually exclusive"),
            (Some(v), false) => Some(BulkApplies::Version(parse_version_spec(v)?)),
            (None, true) => Some(BulkApplies::Generic),
            (None, false) => None,
        };
        if applies.is_none() && tags.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self { applies, tags }))
    }
}

/// Add a version to `applies_to` (design §5): unassessed → that single version;
/// an existing version set gains it; `Generic` absorbs it — a no-op reported as
/// `true` so the caller can warn.
fn add_version(
    applies_to: Option<Applicability>,
    spec: VersionSpec,
) -> (Option<Applicability>, bool) {
    match applies_to {
        None => (
            Some(Applicability::Versions([spec].into_iter().collect())),
            false,
        ),
        Some(Applicability::Versions(mut specs)) => {
            specs.insert(spec);
            (Some(Applicability::Versions(specs)), false)
        }
        Some(Applicability::Generic) => (Some(Applicability::Generic), true),
    }
}

/// Merge a bulk annotation into `ann` (design §5.1), never clobbering existing
/// tags/description/attributes (§9). Returns the merged record and whether a
/// `--ceph-version` was absorbed by an existing `Generic` (a warning).
fn merge_bulk(mut ann: PatchAnnotations, bulk: &BulkAnnotation) -> (PatchAnnotations, bool) {
    let mut absorbed = false;
    match &bulk.applies {
        Some(BulkApplies::Generic) => ann.applies_to = Some(Applicability::Generic),
        Some(BulkApplies::Version(spec)) => {
            let (next, warn) = add_version(ann.applies_to.take(), spec.clone());
            ann.applies_to = next;
            absorbed = warn;
        }
        None => {}
    }
    for t in &bulk.tags {
        ann.tags.insert(t.clone());
    }
    (ann, absorbed)
}

/// Apply a bulk annotation to every blob in `hashes`, merging into each blob's
/// existing record (design §5.1; §9 — import never clobbers). A blob with no
/// record starts from the default. Returns the blobs where a `--ceph-version`
/// was absorbed by an existing `Generic`, for a caller warning.
pub async fn apply_bulk(
    store: &dyn Store,
    hashes: &[Sha256],
    bulk: &BulkAnnotation,
) -> Result<Vec<Sha256>> {
    let mut absorbed = Vec::new();
    for hash in hashes {
        let current = store.get_annotations(hash).await?.unwrap_or_default();
        let (merged, warn) = merge_bulk(current, bulk);
        store.put_annotations(hash, &merged).await?;
        if warn {
            absorbed.push(*hash);
        }
    }
    Ok(absorbed)
}

/// Remove a version from `applies_to` (design §5): drop it from a version set
/// (an empty set normalizes to unassessed); a `None` is a harmless no-op;
/// excluding from `Generic` is a **hard error** rather than a silent no-op
/// that would leave `Generic` matching everything (§9).
fn remove_version(
    applies_to: Option<Applicability>,
    spec: &VersionSpec,
) -> Result<Option<Applicability>> {
    match applies_to {
        None => Ok(None),
        Some(Applicability::Versions(mut specs)) => {
            specs.remove(spec);
            Ok((!specs.is_empty()).then_some(Applicability::Versions(specs)))
        }
        Some(Applicability::Generic) => bail!(
            "this patch is Generic; clear it with `--unassessed`, then set specific \
             `--ceph-version`s before excluding one"
        ),
    }
}

/// The `applies_to` operation of an [`EditAnnotation`]: at most one per
/// invocation (the four flags are mutually exclusive at the CLI, §5.2).
pub enum AppliesEdit {
    Generic,
    Unassessed,
    /// `--ceph-version` (repeatable): add each to the version set.
    Add(Vec<VersionSpec>),
    /// `--unceph-version` (repeatable): remove each from the version set.
    Remove(Vec<VersionSpec>),
}

/// A `--description` edit: set it or clear it (mutually exclusive, §5.2).
pub enum DescriptionEdit {
    Set(String),
    Clear,
}

/// A per-patch annotation edit (design §5.2). The `applies` op is at most one
/// (clap enforces it); the tag/description/attribute facets compose freely.
pub struct EditAnnotation {
    pub applies: Option<AppliesEdit>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub description: Option<DescriptionEdit>,
    pub set_attrs: Vec<(String, String)>,
    pub unset_attrs: Vec<String>,
}

impl EditAnnotation {
    /// Build from `annotate` flags. Parses versions and `key=value` attributes,
    /// rejects `--description` with `--clear-description`, and rejects an edit
    /// with no operation at all (a no-op annotate is a mistake, not a request).
    #[allow(clippy::too_many_arguments)]
    pub fn from_flags(
        ceph_version: Vec<String>,
        unceph_version: Vec<String>,
        generic: bool,
        unassessed: bool,
        add_tags: Vec<String>,
        remove_tags: Vec<String>,
        description: Option<String>,
        clear_description: bool,
        set: Vec<String>,
        unset: Vec<String>,
    ) -> Result<Self> {
        // clap guarantees at most one of these four is present.
        let applies = if generic {
            Some(AppliesEdit::Generic)
        } else if unassessed {
            Some(AppliesEdit::Unassessed)
        } else if !ceph_version.is_empty() {
            Some(AppliesEdit::Add(parse_specs(&ceph_version)?))
        } else if !unceph_version.is_empty() {
            Some(AppliesEdit::Remove(parse_specs(&unceph_version)?))
        } else {
            None
        };
        let description = match (description, clear_description) {
            (Some(_), true) => {
                bail!("--description and --clear-description are mutually exclusive")
            }
            (Some(s), false) => Some(DescriptionEdit::Set(s)),
            (None, true) => Some(DescriptionEdit::Clear),
            (None, false) => None,
        };
        let set_attrs = parse_attrs(&set)?;
        let edit = Self {
            applies,
            add_tags,
            remove_tags,
            description,
            set_attrs,
            unset_attrs: unset,
        };
        if edit.is_noop() {
            bail!("no changes requested; pass at least one annotation flag");
        }
        Ok(edit)
    }

    /// Whether this edit would change nothing.
    fn is_noop(&self) -> bool {
        self.applies.is_none()
            && self.add_tags.is_empty()
            && self.remove_tags.is_empty()
            && self.description.is_none()
            && self.set_attrs.is_empty()
            && self.unset_attrs.is_empty()
    }
}

fn parse_specs(versions: &[String]) -> Result<Vec<VersionSpec>> {
    versions
        .iter()
        .map(|v| Ok(parse_version_spec(v)?))
        .collect()
}

/// Parse `key=value` attribute flags (split on the first `=`; key non-empty).
fn parse_attrs(pairs: &[String]) -> Result<Vec<(String, String)>> {
    pairs
        .iter()
        .map(|kv| match kv.split_once('=') {
            Some(("", _)) => bail!("attribute has an empty key: {kv:?}"),
            Some((k, v)) => Ok((k.to_owned(), v.to_owned())),
            None => bail!("attribute must be key=value: {kv:?}"),
        })
        .collect()
}

/// Apply an edit to `ann` in place (design §5.2). Returns soft warnings (e.g. a
/// `--ceph-version` absorbed by `Generic`); a hard conflict (excluding from
/// `Generic`) is an `Err` — leaving `ann` partially mutated, so the caller must
/// not persist on error.
fn apply_edit_to(ann: &mut PatchAnnotations, edit: &EditAnnotation) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    match &edit.applies {
        Some(AppliesEdit::Generic) => ann.applies_to = Some(Applicability::Generic),
        Some(AppliesEdit::Unassessed) => ann.applies_to = None,
        Some(AppliesEdit::Add(specs)) => {
            for spec in specs {
                let (next, absorbed) = add_version(ann.applies_to.take(), spec.clone());
                ann.applies_to = next;
                if absorbed {
                    warnings.push(format!(
                        "--ceph-version {} ignored: patch is generic",
                        crate::patch::render_spec(spec)
                    ));
                }
            }
        }
        Some(AppliesEdit::Remove(specs)) => {
            for spec in specs {
                ann.applies_to = remove_version(ann.applies_to.take(), spec)?;
            }
        }
        None => {}
    }
    for t in &edit.add_tags {
        ann.tags.insert(t.clone());
    }
    for t in &edit.remove_tags {
        ann.tags.remove(t);
    }
    match &edit.description {
        Some(DescriptionEdit::Set(s)) => ann.description = Some(s.clone()),
        Some(DescriptionEdit::Clear) => ann.description = None,
        None => {}
    }
    for (k, v) in &edit.set_attrs {
        ann.attributes.insert(k.clone(), v.clone());
    }
    for k in &edit.unset_attrs {
        ann.attributes.remove(k);
    }
    Ok(warnings)
}

/// Apply a per-patch edit to one blob's record via a read-modify-write (design
/// §5.2; §9 single-writer). The edit is computed (and validated) before the
/// write, so a hard conflict leaves the stored record untouched. Returns the
/// merged record and any soft warnings.
pub async fn apply_edit(
    store: &dyn Store,
    hash: &Sha256,
    edit: &EditAnnotation,
) -> Result<(PatchAnnotations, Vec<String>)> {
    let mut ann = store.get_annotations(hash).await?.unwrap_or_default();
    let warnings = apply_edit_to(&mut ann, edit)?;
    store.put_annotations(hash, &ann).await?;
    Ok((ann, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt_core::blob_hash;
    use crt_store::ObjectBackedStore;

    fn versions(specs: &[VersionSpec]) -> Applicability {
        Applicability::Versions(specs.iter().cloned().collect())
    }

    #[test]
    fn add_version_transitions() {
        let l = VersionSpec::Line("18.2".to_owned());
        let (a, warn) = add_version(None, l.clone());
        assert!(!warn);
        assert_eq!(a, Some(versions(std::slice::from_ref(&l))));

        // A second version joins the set.
        let e = VersionSpec::Exact("v18.2.0".to_owned());
        let (a2, warn2) = add_version(a, e.clone());
        assert!(!warn2);
        assert_eq!(a2, Some(versions(&[l, e])));

        // Generic absorbs the version and warns (no-op).
        let (g, warn3) = add_version(
            Some(Applicability::Generic),
            VersionSpec::Line("19.2".to_owned()),
        );
        assert!(warn3);
        assert_eq!(g, Some(Applicability::Generic));
    }

    #[test]
    fn from_flags_parses_and_guards() {
        // No flags ⇒ nothing to do.
        assert!(
            BulkAnnotation::from_flags(None, false, vec![])
                .unwrap()
                .is_none()
        );
        // Tag-only is valid (no applicability).
        let t = BulkAnnotation::from_flags(None, false, vec!["rgw".to_owned()])
            .unwrap()
            .unwrap();
        assert!(t.applies.is_none());
        assert_eq!(t.tags, vec!["rgw".to_owned()]);
        // Generic / a version.
        assert!(matches!(
            BulkAnnotation::from_flags(None, true, vec![])
                .unwrap()
                .unwrap()
                .applies,
            Some(BulkApplies::Generic)
        ));
        assert!(matches!(
            BulkAnnotation::from_flags(Some("18.2"), false, vec![])
                .unwrap()
                .unwrap()
                .applies,
            Some(BulkApplies::Version(VersionSpec::Line(_)))
        ));
        // Mutually exclusive, and a bad version, both error.
        assert!(BulkAnnotation::from_flags(Some("18.2"), true, vec![]).is_err());
        assert!(BulkAnnotation::from_flags(Some("nope"), false, vec![]).is_err());
    }

    #[test]
    fn merge_preserves_existing_facets() {
        let mut existing = PatchAnnotations {
            description: Some("keep me".to_owned()),
            ..Default::default()
        };
        existing.tags.insert("existing".to_owned());
        let bulk = BulkAnnotation {
            applies: Some(BulkApplies::Version(VersionSpec::Line("18.2".to_owned()))),
            tags: vec!["rgw".to_owned()],
        };
        let (merged, warn) = merge_bulk(existing, &bulk);
        assert!(!warn);
        assert_eq!(merged.description.as_deref(), Some("keep me"));
        assert!(merged.tags.contains("existing"));
        assert!(merged.tags.contains("rgw"));
        assert!(matches!(
            merged.applies_to,
            Some(Applicability::Versions(_))
        ));
    }

    #[tokio::test]
    async fn apply_bulk_merges_and_warns_on_generic() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"p");
        // A pre-existing Generic record with a tag.
        let mut pre = PatchAnnotations {
            applies_to: Some(Applicability::Generic),
            ..Default::default()
        };
        pre.tags.insert("old".to_owned());
        store.put_annotations(&h, &pre).await.unwrap();

        let bulk = BulkAnnotation {
            applies: Some(BulkApplies::Version(VersionSpec::Line("18.2".to_owned()))),
            tags: vec!["new".to_owned()],
        };
        let absorbed = apply_bulk(&store, &[h], &bulk).await.unwrap();
        assert_eq!(absorbed, vec![h], "version absorbed by Generic warns");

        let got = store.get_annotations(&h).await.unwrap().unwrap();
        assert_eq!(got.applies_to, Some(Applicability::Generic));
        assert!(got.tags.contains("old"), "existing tag preserved");
        assert!(got.tags.contains("new"), "new tag merged");
    }

    #[tokio::test]
    async fn apply_bulk_sets_versions_on_unassessed() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"q");
        let bulk = BulkAnnotation {
            applies: Some(BulkApplies::Version(VersionSpec::Exact(
                "v18.2.0".to_owned(),
            ))),
            tags: vec![],
        };
        let absorbed = apply_bulk(&store, &[h], &bulk).await.unwrap();
        assert!(absorbed.is_empty());
        let got = store.get_annotations(&h).await.unwrap().unwrap();
        assert_eq!(
            got.applies_to,
            Some(versions(&[VersionSpec::Exact("v18.2.0".to_owned())]))
        );
    }

    #[test]
    fn remove_version_transitions() {
        let a = VersionSpec::Line("18.2".to_owned());
        let b = VersionSpec::Exact("v19.2.0".to_owned());
        // Drop one of two.
        let r = remove_version(Some(versions(&[a.clone(), b.clone()])), &a).unwrap();
        assert_eq!(r, Some(versions(std::slice::from_ref(&b))));
        // Dropping the last normalizes to unassessed (None).
        let r = remove_version(Some(versions(std::slice::from_ref(&b))), &b).unwrap();
        assert_eq!(r, None);
        // None is a harmless no-op.
        assert_eq!(remove_version(None, &a).unwrap(), None);
        // Excluding from Generic is a hard error (§9).
        assert!(remove_version(Some(Applicability::Generic), &a).is_err());
    }

    #[test]
    fn parse_attrs_splits_on_first_eq() {
        assert_eq!(
            parse_attrs(&["retire-when=v18.3.0".to_owned()]).unwrap(),
            vec![("retire-when".to_owned(), "v18.3.0".to_owned())]
        );
        // A value may itself contain '='.
        assert_eq!(
            parse_attrs(&["k=a=b".to_owned()]).unwrap(),
            vec![("k".to_owned(), "a=b".to_owned())]
        );
        assert!(parse_attrs(&["noeq".to_owned()]).is_err());
        assert!(parse_attrs(&["=v".to_owned()]).is_err());
    }

    #[test]
    fn from_flags_validates() {
        // A bare annotate (no flags) is a mistake.
        assert!(
            EditAnnotation::from_flags(
                vec![],
                vec![],
                false,
                false,
                vec![],
                vec![],
                None,
                false,
                vec![],
                vec![],
            )
            .is_err()
        );
        // --description with --clear-description conflicts.
        assert!(
            EditAnnotation::from_flags(
                vec![],
                vec![],
                false,
                false,
                vec![],
                vec![],
                Some("x".to_owned()),
                true,
                vec![],
                vec![],
            )
            .is_err()
        );
        // A well-formed edit builds.
        let edit = EditAnnotation::from_flags(
            vec!["18.2".to_owned()],
            vec![],
            false,
            false,
            vec!["rgw".to_owned()],
            vec![],
            None,
            false,
            vec!["k=v".to_owned()],
            vec![],
        )
        .unwrap();
        assert!(matches!(edit.applies, Some(AppliesEdit::Add(_))));
        assert_eq!(edit.set_attrs, vec![("k".to_owned(), "v".to_owned())]);
    }

    #[test]
    fn apply_edit_to_composes_facets() {
        let mut ann = PatchAnnotations::default();
        let edit = EditAnnotation {
            applies: Some(AppliesEdit::Add(vec![VersionSpec::Line("18.2".to_owned())])),
            add_tags: vec!["rgw".to_owned()],
            remove_tags: vec![],
            description: Some(DescriptionEdit::Set("note".to_owned())),
            set_attrs: vec![("retire-when".to_owned(), "v18.3.0".to_owned())],
            unset_attrs: vec![],
        };
        let warnings = apply_edit_to(&mut ann, &edit).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            ann.applies_to,
            Some(versions(&[VersionSpec::Line("18.2".to_owned())]))
        );
        assert!(ann.tags.contains("rgw"));
        assert_eq!(ann.description.as_deref(), Some("note"));
        assert_eq!(
            ann.attributes.get("retire-when").map(String::as_str),
            Some("v18.3.0")
        );

        // Now untag, clear the description, and unset the attribute.
        let undo = EditAnnotation {
            applies: None,
            add_tags: vec![],
            remove_tags: vec!["rgw".to_owned()],
            description: Some(DescriptionEdit::Clear),
            set_attrs: vec![],
            unset_attrs: vec!["retire-when".to_owned()],
        };
        apply_edit_to(&mut ann, &undo).unwrap();
        assert!(ann.tags.is_empty());
        assert!(ann.description.is_none());
        assert!(ann.attributes.is_empty());
    }

    #[test]
    fn apply_edit_to_add_on_generic_warns() {
        let mut ann = PatchAnnotations {
            applies_to: Some(Applicability::Generic),
            ..Default::default()
        };
        let edit = EditAnnotation {
            applies: Some(AppliesEdit::Add(vec![VersionSpec::Line("18.2".to_owned())])),
            add_tags: vec![],
            remove_tags: vec![],
            description: None,
            set_attrs: vec![],
            unset_attrs: vec![],
        };
        let warnings = apply_edit_to(&mut ann, &edit).unwrap();
        assert_eq!(warnings.len(), 1, "absorbed by Generic warns");
        assert_eq!(ann.applies_to, Some(Applicability::Generic));
    }

    #[tokio::test]
    async fn apply_edit_persists_and_guards_generic_removal() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"edit-me");
        // Set generic, then try to exclude a version: hard error, no write.
        store
            .put_annotations(
                &h,
                &PatchAnnotations {
                    applies_to: Some(Applicability::Generic),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let remove = EditAnnotation {
            applies: Some(AppliesEdit::Remove(vec![VersionSpec::Line(
                "18.2".to_owned(),
            )])),
            add_tags: vec![],
            remove_tags: vec![],
            description: None,
            set_attrs: vec![],
            unset_attrs: vec![],
        };
        assert!(apply_edit(&store, &h, &remove).await.is_err());
        // The stored record is untouched (still Generic).
        assert_eq!(
            store.get_annotations(&h).await.unwrap().unwrap().applies_to,
            Some(Applicability::Generic)
        );

        // A tag-only edit persists.
        let tag_edit = EditAnnotation {
            applies: None,
            add_tags: vec!["rgw".to_owned()],
            remove_tags: vec![],
            description: None,
            set_attrs: vec![],
            unset_attrs: vec![],
        };
        let (ann, warnings) = apply_edit(&store, &h, &tag_edit).await.unwrap();
        assert!(warnings.is_empty());
        assert!(ann.tags.contains("rgw"));
        assert!(
            store
                .get_annotations(&h)
                .await
                .unwrap()
                .unwrap()
                .tags
                .contains("rgw")
        );
    }
}
