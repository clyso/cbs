// CRT store — content-addressed blob/meta persistence.
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `Store` trait and a single `object_store`-backed implementation
//! (design §5). The backend — `LocalFileSystem` (dev), `InMemory` (tests),
//! or `AmazonS3` (prod) — is chosen at construction; the key layout and
//! serialization are backend-agnostic.

use std::path::Path as FsPath;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use crt_core::{CrtCoreError, Draft, PatchMeta, ReleaseKey, ReleaseRecord, Sha256};
use futures::TryStreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as StorePath;
use object_store::prefix::PrefixStore;
use object_store::{
    ObjectMeta, ObjectStore, ObjectStoreExt, PutMode, PutPayload, local::LocalFileSystem,
    memory::InMemory,
};
use thiserror::Error;

/// Errors produced by `crt-store`.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("object store: {0}")]
    ObjectStore(#[from] object_store::Error),
    #[error("metadata (de)serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid stored hash: {0}")]
    Hash(#[from] CrtCoreError),
    #[error("corrupt index: {0}")]
    Corrupt(String),
    #[error("release already exists (sealed releases are write-once): {0}")]
    ReleaseExists(String),
    #[error("invalid release key: {0}")]
    InvalidKey(String),
}

type Result<T> = std::result::Result<T, StoreError>;

/// A content-addressed store for patch blobs and their metadata (design §5).
#[async_trait]
pub trait Store: Send + Sync {
    /// Write a patch blob. Content-addressed ⇒ idempotent.
    async fn put_blob(&self, hash: &Sha256, raw: &[u8]) -> Result<()>;
    /// Read a patch blob.
    async fn get_blob(&self, hash: &Sha256) -> Result<Bytes>;
    /// Whether a blob exists.
    async fn has_blob(&self, hash: &Sha256) -> Result<bool>;
    /// Write patch metadata (JSON).
    async fn put_meta(&self, hash: &Sha256, meta: &PatchMeta) -> Result<()>;
    /// Read patch metadata.
    async fn get_meta(&self, hash: &Sha256) -> Result<PatchMeta>;
    /// Look up the blob hash recorded for a `patch_id`, if any (design §4).
    async fn get_patch_id(&self, patch_id: &str) -> Result<Option<Sha256>>;
    /// Record that `patch_id` maps to `hash` (the equivalence index).
    async fn put_patch_id(&self, patch_id: &str, hash: &Sha256) -> Result<()>;

    /// Write (or overwrite) a mutable draft (design §5). Drafts are **not**
    /// write-once — `seal` consumes one and any operator can pick it up.
    async fn put_draft(&self, key: &ReleaseKey, draft: &Draft) -> Result<()>;
    /// Read a draft.
    async fn get_draft(&self, key: &ReleaseKey) -> Result<Draft>;
    /// List the keys of all drafts.
    async fn list_drafts(&self) -> Result<Vec<ReleaseKey>>;
    /// Delete a draft (e.g. once it has been sealed).
    async fn delete_draft(&self, key: &ReleaseKey) -> Result<()>;

    /// Write a sealed release **write-once**: an existing key is refused
    /// (design §5). Returns [`StoreError::ReleaseExists`] on a duplicate.
    async fn put_release(&self, key: &ReleaseKey, record: &ReleaseRecord) -> Result<()>;
    /// Read a sealed release record.
    async fn get_release(&self, key: &ReleaseKey) -> Result<ReleaseRecord>;
    /// List the keys of all sealed releases.
    async fn list_releases(&self) -> Result<Vec<ReleaseKey>>;

    /// Write a notes template, content-addressed by digest. Idempotent.
    async fn put_template(&self, digest: &Sha256, bytes: &[u8]) -> Result<()>;
    /// Read a notes template by digest.
    async fn get_template(&self, digest: &Sha256) -> Result<Bytes>;
}

fn blob_path(hash: &Sha256) -> StorePath {
    StorePath::from(format!("patches/blobs/sha256/{}", hash.to_hex()))
}

fn meta_path(hash: &Sha256) -> StorePath {
    StorePath::from(format!("patches/meta/sha256/{}.json", hash.to_hex()))
}

fn patch_id_path(patch_id: &str) -> StorePath {
    StorePath::from(format!("patches/by-patch-id/{patch_id}"))
}

const DRAFTS_PREFIX: &str = "drafts";
const RELEASES_PREFIX: &str = "releases";

/// Reject a key that would not round-trip through the store's path layout: each
/// segment must be non-empty and free of `/`, otherwise the object could be
/// written but never recovered by `list` (the inverse of `parse_release_key`).
fn validate_key(key: &ReleaseKey) -> Result<()> {
    for (field, value) in [
        ("namespace", &key.namespace),
        ("channel", &key.channel),
        ("name", &key.name),
    ] {
        if value.is_empty() || value.contains('/') {
            return Err(StoreError::InvalidKey(format!(
                "{field} must be non-empty and contain no '/': {value:?}"
            )));
        }
    }
    Ok(())
}

fn draft_path(key: &ReleaseKey) -> StorePath {
    StorePath::from(format!(
        "{DRAFTS_PREFIX}/{}/{}/{}.json",
        key.namespace, key.channel, key.name
    ))
}

fn release_record_path(key: &ReleaseKey) -> StorePath {
    StorePath::from(format!(
        "{RELEASES_PREFIX}/{}/{}/{}.json",
        key.namespace, key.channel, key.name
    ))
}

fn template_path(digest: &Sha256) -> StorePath {
    StorePath::from(format!("templates/sha256/{}", digest.to_hex()))
}

/// Recover a `ReleaseKey` from a `<prefix>/<ns>/<channel>/<name>.json` path
/// (the inverse of `draft_path`/`release_record_path`). Non-conforming keys
/// under the prefix are skipped (returns `None`).
fn parse_release_key(loc: &StorePath, prefix: &str) -> Option<ReleaseKey> {
    let parts: Vec<String> = loc.parts().map(|p| p.as_ref().to_owned()).collect();
    if parts.len() != 4 || parts[0] != prefix {
        return None;
    }
    let name = parts[3].strip_suffix(".json")?.to_owned();
    Some(ReleaseKey {
        namespace: parts[1].clone(),
        channel: parts[2].clone(),
        name,
    })
}

/// A `Store` backed by any `object_store::ObjectStore`.
pub struct ObjectBackedStore {
    inner: Arc<dyn ObjectStore>,
}

impl ObjectBackedStore {
    /// Wrap an existing object store.
    #[must_use]
    pub fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self { inner }
    }

    /// Back the store with a local filesystem directory (dev). The directory
    /// is created if absent.
    pub fn local(root: impl AsRef<FsPath>) -> Result<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        let fs = LocalFileSystem::new_with_prefix(root)?;
        Ok(Self {
            inner: Arc::new(fs),
        })
    }

    /// Back the store with an in-memory object store (tests).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(InMemory::new()),
        }
    }

    /// Back the store with S3 (or an S3-compatible endpoint). Credentials come
    /// from the secrets file (design §9); the optional `prefix` is applied to
    /// every key. No live connection is made here.
    pub fn s3(s: &S3Settings) -> Result<Self> {
        let s3 = AmazonS3Builder::new()
            .with_endpoint(&s.endpoint)
            .with_region(&s.region)
            .with_bucket_name(&s.bucket)
            .with_access_key_id(&s.access_key_id)
            .with_secret_access_key(&s.secret_access_key)
            .with_allow_http(s.endpoint.starts_with("http://"))
            .build()?;
        let inner: Arc<dyn ObjectStore> = if s.prefix.is_empty() {
            Arc::new(s3)
        } else {
            Arc::new(PrefixStore::new(s3, StorePath::from(s.prefix.as_str())))
        };
        Ok(Self { inner })
    }

    /// List the release keys under a `drafts/` or `releases/` prefix by
    /// enumerating the store. The prefix is the source of truth — no separate
    /// index file is maintained (design §5's `index/releases.json` is a
    /// re-derivable cache, deferred to the service era).
    async fn list_keys(&self, prefix: &str) -> Result<Vec<ReleaseKey>> {
        let metas: Vec<ObjectMeta> = self
            .inner
            .list(Some(&StorePath::from(prefix)))
            .try_collect()
            .await?;
        Ok(metas
            .iter()
            .filter_map(|m| parse_release_key(&m.location, prefix))
            .collect())
    }
}

/// Connection settings for an S3 (or S3-compatible) backend. Endpoint/bucket
/// come from `crt.config.yaml`; credentials from `crt.secrets.yaml` (design §9).
pub struct S3Settings {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// Key prefix applied to every object (e.g. `crt/`); empty for none.
    pub prefix: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[async_trait]
impl Store for ObjectBackedStore {
    async fn put_blob(&self, hash: &Sha256, raw: &[u8]) -> Result<()> {
        self.inner
            .put(&blob_path(hash), PutPayload::from(raw.to_vec()))
            .await?;
        Ok(())
    }

    async fn get_blob(&self, hash: &Sha256) -> Result<Bytes> {
        let res = self.inner.get(&blob_path(hash)).await?;
        Ok(res.bytes().await?)
    }

    async fn has_blob(&self, hash: &Sha256) -> Result<bool> {
        match self.inner.head(&blob_path(hash)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    async fn put_meta(&self, hash: &Sha256, meta: &PatchMeta) -> Result<()> {
        let json = serde_json::to_vec_pretty(meta)?;
        self.inner
            .put(&meta_path(hash), PutPayload::from(json))
            .await?;
        Ok(())
    }

    async fn get_meta(&self, hash: &Sha256) -> Result<PatchMeta> {
        let res = self.inner.get(&meta_path(hash)).await?;
        let bytes = res.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn get_patch_id(&self, patch_id: &str) -> Result<Option<Sha256>> {
        match self.inner.get(&patch_id_path(patch_id)).await {
            Ok(res) => {
                let bytes = res.bytes().await?;
                let hex = String::from_utf8(bytes.to_vec())
                    .map_err(|_| StoreError::Corrupt("patch-id index not UTF-8".to_owned()))?;
                Ok(Some(Sha256::try_from(hex.trim().to_owned())?))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn put_patch_id(&self, patch_id: &str, hash: &Sha256) -> Result<()> {
        self.inner
            .put(
                &patch_id_path(patch_id),
                PutPayload::from(hash.to_hex().into_bytes()),
            )
            .await?;
        Ok(())
    }

    async fn put_draft(&self, key: &ReleaseKey, draft: &Draft) -> Result<()> {
        validate_key(key)?;
        let json = serde_json::to_vec_pretty(draft)?;
        self.inner
            .put(&draft_path(key), PutPayload::from(json))
            .await?;
        Ok(())
    }

    async fn get_draft(&self, key: &ReleaseKey) -> Result<Draft> {
        let res = self.inner.get(&draft_path(key)).await?;
        let bytes = res.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn list_drafts(&self) -> Result<Vec<ReleaseKey>> {
        self.list_keys(DRAFTS_PREFIX).await
    }

    async fn delete_draft(&self, key: &ReleaseKey) -> Result<()> {
        self.inner.delete(&draft_path(key)).await?;
        Ok(())
    }

    async fn put_release(&self, key: &ReleaseKey, record: &ReleaseRecord) -> Result<()> {
        validate_key(key)?;
        let json = serde_json::to_vec_pretty(record)?;
        let path = release_record_path(key);
        // `PutMode::Create` is an atomic write-once: it fails rather than
        // overwriting an existing sealed release (design §5) — no TOCTOU gap.
        match self
            .inner
            .put_opts(&path, PutPayload::from(json), PutMode::Create.into())
            .await
        {
            Ok(_) => Ok(()),
            Err(object_store::Error::AlreadyExists { .. }) => {
                Err(StoreError::ReleaseExists(path.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn get_release(&self, key: &ReleaseKey) -> Result<ReleaseRecord> {
        let res = self.inner.get(&release_record_path(key)).await?;
        let bytes = res.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn list_releases(&self) -> Result<Vec<ReleaseKey>> {
        self.list_keys(RELEASES_PREFIX).await
    }

    async fn put_template(&self, digest: &Sha256, bytes: &[u8]) -> Result<()> {
        self.inner
            .put(&template_path(digest), PutPayload::from(bytes.to_vec()))
            .await?;
        Ok(())
    }

    async fn get_template(&self, digest: &Sha256) -> Result<Bytes> {
        let res = self.inner.get(&template_path(digest)).await?;
        Ok(res.bytes().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt_core::{Identity, Provenance, blob_hash};

    fn sample_meta(hash: Sha256) -> PatchMeta {
        PatchMeta {
            blob_hash: hash,
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
        }
    }

    #[tokio::test]
    async fn blob_round_trip_in_memory() {
        let store = ObjectBackedStore::in_memory();
        let raw = b"a patch";
        let h = blob_hash(raw);
        assert!(!store.has_blob(&h).await.unwrap());
        store.put_blob(&h, raw).await.unwrap();
        assert!(store.has_blob(&h).await.unwrap());
        assert_eq!(&store.get_blob(&h).await.unwrap()[..], raw);
    }

    #[tokio::test]
    async fn meta_round_trip_in_memory() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"x");
        let meta = sample_meta(h);
        store.put_meta(&h, &meta).await.unwrap();
        assert_eq!(store.get_meta(&h).await.unwrap(), meta);
    }

    #[tokio::test]
    async fn patch_id_index_round_trip() {
        let store = ObjectBackedStore::in_memory();
        let h = blob_hash(b"p");
        assert!(store.get_patch_id("pid-123").await.unwrap().is_none());
        store.put_patch_id("pid-123", &h).await.unwrap();
        assert_eq!(store.get_patch_id("pid-123").await.unwrap(), Some(h));
    }

    fn sample_key(name: &str) -> ReleaseKey {
        ReleaseKey {
            namespace: "clyso-enterprise".to_owned(),
            channel: "ces".to_owned(),
            name: name.to_owned(),
        }
    }

    fn sample_header(name: &str) -> crt_core::ReleaseHeader {
        crt_core::ReleaseHeader {
            product: "ceph".to_owned(),
            namespace: "clyso-enterprise".to_owned(),
            channel: "ces".to_owned(),
            name: name.to_owned(),
            base_ref: "v18.2.0".to_owned(),
            created: "2026-06-21T00:00:00+00:00".to_owned(),
            author: Identity {
                name: "Releaser".to_owned(),
                email: "rel@example.com".to_owned(),
            },
        }
    }

    fn sample_draft(name: &str) -> Draft {
        Draft {
            release: sample_header(name),
            entries: vec![],
            known_issues: vec![],
            upgrade_notes: None,
        }
    }

    fn sample_record(name: &str) -> ReleaseRecord {
        let manifest = crt_core::Manifest {
            schema_version: 1,
            release: sample_header(name),
            entries: vec![],
            known_issues: vec![],
            upgrade_notes: None,
            branding: crt_core::Branding {
                display_name: "Clyso Enterprise Storage".to_owned(),
                blurb: "b".to_owned(),
                footer: "f".to_owned(),
            },
            render: crt_core::RenderSpec {
                minijinja_version: "2.21.0".to_owned(),
                template_digest: blob_hash(b"template"),
            },
        };
        let digest = crt_core::digest(&manifest).unwrap();
        ReleaseRecord {
            manifest,
            digest,
            signature: crt_core::ArmoredSignature("-----BEGIN PGP SIGNATURE-----".to_owned()),
        }
    }

    #[tokio::test]
    async fn draft_round_trips_and_overwrites() {
        let store = ObjectBackedStore::in_memory();
        let key = sample_key("ces-v18.2.0");
        let mut draft = sample_draft("ces-v18.2.0");
        store.put_draft(&key, &draft).await.unwrap();
        assert_eq!(store.get_draft(&key).await.unwrap(), draft);

        // Drafts are mutable (serial handoff): a second put overwrites.
        draft.upgrade_notes = Some("now with notes".to_owned());
        store.put_draft(&key, &draft).await.unwrap();
        assert_eq!(
            store
                .get_draft(&key)
                .await
                .unwrap()
                .upgrade_notes
                .as_deref(),
            Some("now with notes")
        );

        store.delete_draft(&key).await.unwrap();
        assert!(store.get_draft(&key).await.is_err());
    }

    #[tokio::test]
    async fn release_is_write_once() {
        let store = ObjectBackedStore::in_memory();
        let key = sample_key("ces-v18.2.0");
        let record = sample_record("ces-v18.2.0");
        store.put_release(&key, &record).await.unwrap();
        assert_eq!(store.get_release(&key).await.unwrap(), record);

        // A second write of the same key is refused (design §5).
        let err = store.put_release(&key, &record).await.unwrap_err();
        assert!(matches!(err, StoreError::ReleaseExists(_)));
    }

    #[tokio::test]
    async fn list_keys_reflect_drafts_and_releases() {
        let store = ObjectBackedStore::in_memory();
        store
            .put_draft(&sample_key("ces-v18.2.0"), &sample_draft("ces-v18.2.0"))
            .await
            .unwrap();
        store
            .put_draft(&sample_key("ces-v18.2.1"), &sample_draft("ces-v18.2.1"))
            .await
            .unwrap();
        store
            .put_release(&sample_key("ces-v18.1.0"), &sample_record("ces-v18.1.0"))
            .await
            .unwrap();

        let mut drafts = store.list_drafts().await.unwrap();
        drafts.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            drafts,
            vec![sample_key("ces-v18.2.0"), sample_key("ces-v18.2.1")]
        );

        // Listing the draft prefix must not pick up the sealed release.
        let releases = store.list_releases().await.unwrap();
        assert_eq!(releases, vec![sample_key("ces-v18.1.0")]);
    }

    #[tokio::test]
    async fn template_round_trips() {
        let store = ObjectBackedStore::in_memory();
        let bytes = b"{{ release.name }} notes template";
        let digest = blob_hash(bytes);
        store.put_template(&digest, bytes).await.unwrap();
        assert_eq!(&store.get_template(&digest).await.unwrap()[..], bytes);
    }

    #[tokio::test]
    async fn release_round_trips_local_fs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectBackedStore::local(dir.path()).unwrap();
        let key = sample_key("ces-v18.2.0");
        let record = sample_record("ces-v18.2.0");
        store.put_release(&key, &record).await.unwrap();
        assert_eq!(store.get_release(&key).await.unwrap(), record);
        assert_eq!(store.list_releases().await.unwrap(), vec![key]);
    }

    #[tokio::test]
    async fn list_round_trips_through_a_prefix_store() {
        // An S3 deployment wraps the backend in a `PrefixStore` (e.g. `crt/`).
        // `list` must still recover keys: `PrefixStore` strips the prefix from
        // the locations it returns, so `parse_release_key` sees the unprefixed
        // `<prefix>/<ns>/<channel>/<name>.json` form.
        let inner = PrefixStore::new(InMemory::new(), StorePath::from("crt/"));
        let store = ObjectBackedStore::new(Arc::new(inner));
        let draft_key = sample_key("ces-v18.2.0");
        let release_key = sample_key("ces-v18.1.0");
        store
            .put_draft(&draft_key, &sample_draft("ces-v18.2.0"))
            .await
            .unwrap();
        store
            .put_release(&release_key, &sample_record("ces-v18.1.0"))
            .await
            .unwrap();
        assert_eq!(store.list_drafts().await.unwrap(), vec![draft_key]);
        assert_eq!(store.list_releases().await.unwrap(), vec![release_key]);
    }

    #[tokio::test]
    async fn invalid_keys_are_refused() {
        let store = ObjectBackedStore::in_memory();
        // A `/` in a key segment would write an object `list` could never
        // recover (it splits into the wrong number of parts).
        let slash = ReleaseKey {
            namespace: "clyso-enterprise".to_owned(),
            channel: "ces".to_owned(),
            name: "has/slash".to_owned(),
        };
        assert!(matches!(
            store
                .put_draft(&slash, &sample_draft("x"))
                .await
                .unwrap_err(),
            StoreError::InvalidKey(_)
        ));
        // An empty segment is likewise rejected before any write.
        let empty = ReleaseKey {
            namespace: String::new(),
            channel: "ces".to_owned(),
            name: "n".to_owned(),
        };
        assert!(matches!(
            store
                .put_release(&empty, &sample_record("n"))
                .await
                .unwrap_err(),
            StoreError::InvalidKey(_)
        ));
    }

    #[tokio::test]
    async fn blob_round_trip_local_fs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectBackedStore::local(dir.path()).unwrap();
        let raw = b"local patch";
        let h = blob_hash(raw);
        store.put_blob(&h, raw).await.unwrap();
        assert_eq!(&store.get_blob(&h).await.unwrap()[..], raw);
    }

    #[test]
    fn s3_settings_build_a_store() {
        // Constructs the client (no network); validates the builder wiring.
        let settings = S3Settings {
            endpoint: "http://localhost:9000".to_owned(),
            region: "us-east-1".to_owned(),
            bucket: "bucket".to_owned(),
            prefix: "crt/".to_owned(),
            access_key_id: "id".to_owned(),
            secret_access_key: "key".to_owned(),
        };
        assert!(ObjectBackedStore::s3(&settings).is_ok());
    }

    /// Real S3 round-trip. Opt-in: set `CRT_TEST_S3_*` and run with
    /// `cargo test -p crt-store -- --ignored`. Never runs in plain `cargo test`
    /// — there is no minio dependency (design §5).
    #[tokio::test]
    #[ignore = "requires a real S3 endpoint; set CRT_TEST_S3_* and run --ignored"]
    async fn s3_blob_round_trip_real() {
        let settings = S3Settings {
            endpoint: std::env::var("CRT_TEST_S3_ENDPOINT").expect("CRT_TEST_S3_ENDPOINT"),
            region: std::env::var("CRT_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned()),
            bucket: std::env::var("CRT_TEST_S3_BUCKET").expect("CRT_TEST_S3_BUCKET"),
            prefix: "crt-test/".to_owned(),
            access_key_id: std::env::var("CRT_TEST_S3_ACCESS_KEY").expect("CRT_TEST_S3_ACCESS_KEY"),
            secret_access_key: std::env::var("CRT_TEST_S3_SECRET_KEY")
                .expect("CRT_TEST_S3_SECRET_KEY"),
        };
        let store = ObjectBackedStore::s3(&settings).unwrap();
        let raw = b"real s3 patch";
        let h = blob_hash(raw);
        store.put_blob(&h, raw).await.unwrap();
        assert_eq!(&store.get_blob(&h).await.unwrap()[..], raw);
    }
}
