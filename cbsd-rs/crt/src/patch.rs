// crt — imported-patch introspection (`patch list` / `patch info`).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only introspection of the content-addressed patch store (design/plan
//! 002): `crt patch list` and `crt patch info`. The functions return data; the
//! CLI renders it either as text (the helpers here) or as JSON
//! (`serde_json::to_string_pretty` in `main`).

use anyhow::Result;
use crt_core::PatchMeta;
use crt_store::Store;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crt_core::{Identity, Provenance, Sha256, blob_hash};
    use crt_store::ObjectBackedStore;

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
}
