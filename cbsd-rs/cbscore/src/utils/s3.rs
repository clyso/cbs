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

//! The S3 object-store client (design 005). Source: `cbscore/utils/s3.py`.
//!
//! `aws-sdk-s3` replaces `aioboto3`. Every operation resolves credentials via
//! [`SecretsMgr::s3_creds`] and talks to a **custom endpoint** (MinIO / Ceph
//! RGW), not AWS (invariant 9 / review H4): explicit static credentials (never
//! the AWS default provider chain), the secrets-resolved hostname as
//! `endpoint_url` (scheme-normalised), a placeholder region, and
//! **`force_path_style(true)`**. TLS uses the ring-backed rustls connector
//! (musl-clean, design 012) over the **system trust store** (rustls-native-certs
//! — the builder image ships `ca-certificates`), so a private MinIO/RGW CA is
//! honored when installed system-wide.
//!
//! This slice lands the **write** path and the **download primitive**; `s3_list`
//! and the `releases/` operations land with their consumers (M3/C4b/C6).

use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::ObjectCannedAcl;
use aws_smithy_http_client::{Builder, tls};
use camino::Utf8PathBuf;

use crate::utils::secrets::{SecretsError, SecretsMgr};

/// A local file to upload and its S3 destination key (`s3.py:34-42`).
pub struct S3FileLocator {
    pub src: Utf8PathBuf,
    pub dst: String,
    pub name: String,
}

/// An error from an S3 operation (design 005; lives here per 002). Wrapped SDK
/// errors never carry credentials — only the endpoint/key context.
#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    /// Resolving the S3 credentials failed.
    #[error("error obtaining S3 credentials")]
    Creds(#[source] SecretsError),
    /// Uploading a string/JSON object failed.
    #[error("error uploading object to '{key}'")]
    Upload {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Reading a local file for upload failed.
    #[error("error reading file '{path}' for upload")]
    FileRead {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Uploading a local file failed.
    #[error("error uploading file '{name}' to '{dst}'")]
    UploadFile {
        name: String,
        dst: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Downloading an object failed (a missing object is `Ok(None)`, not this).
    #[error("error downloading object from '{key}'")]
    Download {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Reading the body stream of a downloaded object failed.
    #[error("error reading the body of '{key}'")]
    Body {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// A downloaded object's bytes are not valid UTF-8.
    #[error("object '{key}' is not valid UTF-8")]
    NotUtf8 {
        key: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
    /// The downloaded object's content type did not match the expected one.
    #[error("unexpected content type '{got}' for '{key}' (wanted '{want}')")]
    ContentType {
        key: String,
        got: String,
        want: String,
    },
}

/// Build an S3 client for `hostname` with explicit static credentials, a
/// scheme-normalised custom endpoint, a placeholder region, and path-style
/// addressing — the MinIO/RGW shape (design 005). A fresh client per call,
/// matching Python's per-call session.
fn s3_client(hostname: &str, access_id: &str, secret_id: &str) -> Client {
    // Ring-backed rustls HTTPS connector (design 012): aws-sdk-s3's
    // `default-https-client` would link aws-lc-rs, so build it explicitly. The
    // default `TrustStore` enables native (system) roots.
    let http_client = Builder::new()
        .tls_provider(tls::Provider::Rustls(
            tls::rustls_provider::CryptoMode::Ring,
        ))
        .build_https();
    let creds = Credentials::new(access_id, secret_id, None, None, "cbscore-secrets");
    let config = aws_sdk_s3::Config::builder()
        .http_client(http_client)
        .region(Region::new("us-east-1"))
        .endpoint_url(normalize_endpoint(hostname))
        .credentials_provider(creds)
        .force_path_style(true)
        .behavior_version(BehaviorVersion::latest())
        .build();
    Client::from_conf(config)
}

/// Prefix `https://` when the hostname carries no scheme (`s3.py:97-98`; the
/// guard is `startswith("http")`, so an `http://` endpoint is left as-is).
fn normalize_endpoint(hostname: &str) -> String {
    if hostname.starts_with("http") {
        hostname.to_string()
    } else {
        format!("https://{hostname}")
    }
}

/// Upload a string object to `bucket/key` with `content_type` (`s3.py:70-111`).
pub async fn s3_upload_str_obj(
    secrets: &SecretsMgr,
    url: &str,
    bucket: &str,
    key: &str,
    body: String,
    content_type: &str,
) -> Result<(), S3Error> {
    let (hostname, access_id, secret_id) = secrets.s3_creds(url).await.map_err(S3Error::Creds)?;
    let client = s3_client(&hostname, &access_id, &secret_id);
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(body.into_bytes()))
        .content_type(content_type)
        .send()
        .await
        .map_err(|e| S3Error::Upload {
            key: key.to_string(),
            source: Box::new(e),
        })?;
    Ok(())
}

/// Upload a JSON string object (content type `application/json`, `s3.py:196`).
pub async fn s3_upload_json(
    secrets: &SecretsMgr,
    url: &str,
    bucket: &str,
    key: &str,
    body: String,
) -> Result<(), S3Error> {
    s3_upload_str_obj(secrets, url, bucket, key, body, "application/json").await
}

/// Upload a list of local files to `bucket`, optionally `public-read`
/// (`s3.py:219-285`). boto3's `upload_file` auto-multiparts; the port uses a
/// single PUT with a file-backed [`ByteStream`] — RPMs are well under the
/// single-PUT ceiling (design 005; multipart is a noted follow-up only).
pub async fn s3_upload_files(
    secrets: &SecretsMgr,
    url: &str,
    bucket: &str,
    file_locs: &[S3FileLocator],
    public: bool,
) -> Result<(), S3Error> {
    let (hostname, access_id, secret_id) = secrets.s3_creds(url).await.map_err(S3Error::Creds)?;
    let client = s3_client(&hostname, &access_id, &secret_id);
    for loc in file_locs {
        let body = ByteStream::from_path(&loc.src)
            .await
            .map_err(|e| S3Error::FileRead {
                path: loc.src.to_string(),
                source: Box::new(e),
            })?;
        let mut req = client.put_object().bucket(bucket).key(&loc.dst).body(body);
        if public {
            req = req.acl(ObjectCannedAcl::PublicRead);
        }
        req.send().await.map_err(|e| S3Error::UploadFile {
            name: loc.name.clone(),
            dst: loc.dst.clone(),
            source: Box::new(e),
        })?;
    }
    Ok(())
}

/// Download a string object from `bucket/key`; a missing object returns
/// **`None`**, not an error (`s3.py:114-193`). When `content_type` is given, a
/// mismatch errors. The single `get_object` carries both the content type and
/// the body, replacing Python's separate HEAD+GET.
pub async fn s3_download_str_obj(
    secrets: &SecretsMgr,
    url: &str,
    bucket: &str,
    key: &str,
    content_type: Option<&str>,
) -> Result<Option<String>, S3Error> {
    let (hostname, access_id, secret_id) = secrets.s3_creds(url).await.map_err(S3Error::Creds)?;
    let client = s3_client(&hostname, &access_id, &secret_id);

    let resp = match client.get_object().bucket(bucket).key(key).send().await {
        Ok(resp) => resp,
        Err(err) => {
            // 404 / NoSuchKey → None (design 005); anything else is an error.
            let not_found = err.as_service_error().is_some_and(|e| e.is_no_such_key())
                || err
                    .raw_response()
                    .is_some_and(|r| r.status().as_u16() == 404);
            if not_found {
                return Ok(None);
            }
            return Err(S3Error::Download {
                key: key.to_string(),
                source: Box::new(err),
            });
        }
    };

    if let Some(want) = content_type {
        let got = resp.content_type();
        if got != Some(want) {
            return Err(S3Error::ContentType {
                key: key.to_string(),
                got: got.unwrap_or_default().to_string(),
                want: want.to_string(),
            });
        }
    }

    let bytes = resp
        .body
        .collect()
        .await
        .map_err(|e| S3Error::Body {
            key: key.to_string(),
            source: Box::new(e),
        })?
        .into_bytes();
    let text = String::from_utf8(bytes.to_vec()).map_err(|source| S3Error::NotUtf8 {
        key: key.to_string(),
        source,
    })?;
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use cbscore_types::{Secrets, StorageSecret};

    #[test]
    fn normalize_endpoint_adds_https_only_when_schemeless() {
        assert_eq!(
            normalize_endpoint("s3.example.com"),
            "https://s3.example.com"
        );
        assert_eq!(
            normalize_endpoint("https://s3.example.com"),
            "https://s3.example.com"
        );
        // `startswith("http")` leaves an http:// endpoint untouched (s3.py:97).
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:9000"),
            "http://127.0.0.1:9000"
        );
    }

    /// Build a [`SecretsMgr`] with a single plain-S3 storage secret keyed by
    /// `url`.
    fn mgr_with_s3(url: &str, access_id: &str, secret_id: &str) -> SecretsMgr {
        let storage = BTreeMap::from([(
            url.to_string(),
            StorageSecret::PlainS3 {
                access_id: access_id.to_string(),
                secret_id: secret_id.to_string(),
            },
        )]);
        SecretsMgr::new(Secrets {
            schema_version: 1,
            git: BTreeMap::new(),
            storage,
            sign: BTreeMap::new(),
            registry: BTreeMap::new(),
        })
    }

    /// MinIO parity round-trip (the H4 gate): put then get an object over a
    /// custom endpoint with injected creds and path-style addressing, and a
    /// missing key returns `None`. Ignored — needs a live MinIO/RGW:
    ///
    /// ```text
    /// podman run -p 9000:9000 -e MINIO_ROOT_USER=minioadmin \
    ///   -e MINIO_ROOT_PASSWORD=minioadmin minio/minio server /data
    /// mc alias set local http://127.0.0.1:9000 minioadmin minioadmin
    /// mc mb local/cbs-test
    /// export CBS_TEST_S3_URL=http://127.0.0.1:9000 \
    ///   CBS_TEST_S3_ACCESS=minioadmin CBS_TEST_S3_SECRET=minioadmin \
    ///   CBS_TEST_S3_BUCKET=cbs-test
    /// cargo test -p cbscore --lib -- --ignored s3_round_trip
    /// ```
    #[tokio::test]
    #[ignore = "requires a live MinIO/RGW endpoint (CBS_TEST_S3_* env)"]
    async fn s3_round_trip_put_get_and_missing_key() {
        let url = std::env::var("CBS_TEST_S3_URL").expect("CBS_TEST_S3_URL");
        let access = std::env::var("CBS_TEST_S3_ACCESS").expect("CBS_TEST_S3_ACCESS");
        let secret = std::env::var("CBS_TEST_S3_SECRET").expect("CBS_TEST_S3_SECRET");
        let bucket = std::env::var("CBS_TEST_S3_BUCKET").expect("CBS_TEST_S3_BUCKET");
        let mgr = mgr_with_s3(&url, &access, &secret);

        // A missing key downloads as None, not an error.
        let missing = s3_download_str_obj(&mgr, &url, &bucket, "cbs/does-not-exist", None)
            .await
            .expect("download missing key");
        assert_eq!(missing, None);

        // Upload then download the same JSON object round-trips.
        let key = "cbs/round-trip.json";
        let body = r#"{"hello":"s3"}"#.to_string();
        s3_upload_json(&mgr, &url, &bucket, key, body.clone())
            .await
            .expect("upload json");
        let got = s3_download_str_obj(&mgr, &url, &bucket, key, Some("application/json"))
            .await
            .expect("download json");
        assert_eq!(got.as_deref(), Some(body.as_str()));

        // Upload a real file via the file path (exercises `ByteStream::from_path`
        // and, for `public=true`, the `public-read` ACL branch), then download
        // it back.
        let dir = tempfile::tempdir().unwrap();
        let src = camino::Utf8Path::from_path(dir.path())
            .unwrap()
            .join("artifact.txt");
        std::fs::write(&src, b"rpm-bytes").unwrap();
        let loc = S3FileLocator {
            src,
            dst: "cbs/artifact.txt".to_string(),
            name: "artifact.txt".to_string(),
        };
        for public in [false, true] {
            s3_upload_files(&mgr, &url, &bucket, std::slice::from_ref(&loc), public)
                .await
                .expect("upload file");
        }
        let got = s3_download_str_obj(&mgr, &url, &bucket, &loc.dst, None)
            .await
            .expect("download uploaded file");
        assert_eq!(got.as_deref(), Some("rpm-bytes"));
    }
}
