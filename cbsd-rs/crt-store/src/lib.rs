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
use crt_core::{PatchMeta, Sha256};
use object_store::path::Path as StorePath;
use object_store::{
    ObjectStore, ObjectStoreExt, PutPayload, local::LocalFileSystem, memory::InMemory,
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
}

fn blob_path(hash: &Sha256) -> StorePath {
    StorePath::from(format!("patches/blobs/sha256/{}", hash.to_hex()))
}

fn meta_path(hash: &Sha256) -> StorePath {
    StorePath::from(format!("patches/meta/sha256/{}.json", hash.to_hex()))
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
    async fn blob_round_trip_local_fs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectBackedStore::local(dir.path()).unwrap();
        let raw = b"local patch";
        let h = blob_hash(raw);
        store.put_blob(&h, raw).await.unwrap();
        assert_eq!(&store.get_blob(&h).await.unwrap()[..], raw);
    }
}
