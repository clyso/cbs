# 005 — Storage (S3) & releases

This is the reference design for the S3 storage client and the release S3
operations of the `cbscore` library: the object-store client (`aws-sdk-s3`
replacing `aioboto3`), and the read/write/list operations over release and
component-release descriptors. It owns these as the single source of truth. Read
002 for the `ReleaseDesc` / `ReleaseComponent` types (defined there, referenced
here), 004 for the `SecretsMgr.s3_creds` resolver this layer consumes, and 001
for the schema policy and correctness invariant 9.

Source of truth: `cbscore/utils/s3.py` (the client), `cbscore/releases/s3.py`
(release operations). The `ReleaseDesc` wire types live in
`cbscore/releases/desc.py` (→ 002); `releases/utils.py`'s
`get_component_release_rpm` is a component-script helper consumed by the builder
upload stage and is specified in 007, not here.

## S3 client

Source: `utils/s3.py`. Every operation takes a `&SecretsMgr` and an S3 `url`,
resolves credentials via `secrets.s3_creds(url)` (004) →
`(hostname, access_id, secret_id)`, and talks to a **custom endpoint** (MinIO /
Ceph RGW), not AWS. This is the concrete landing of correctness invariant 9 /
review H4.

Client construction (resolves H4):

- **Credentials are injected explicitly** from the secrets store as a static
  provider (`aws_sdk_s3::config::Credentials::new(access_id, secret_id, …)`),
  **never** the AWS default provider chain. Python builds the session with
  explicit `aws_access_key_id`/`aws_secret_access_key` (`s3.py:92-95`).
- **`endpoint_url` is the secrets-resolved hostname**, scheme-normalized (prefix
  `https://` when it has no scheme, matching `s3.py:97-98`).
- **Region**: Python passes `region=None`; `aws-sdk-s3` requires a region, so
  the port sets a fixed placeholder (e.g. `us-east-1`) — irrelevant for a custom
  endpoint but required by the SDK.
- **Path-style addressing**: set `force_path_style(true)`. `aws-sdk-s3` defaults
  to virtual-hosted-style, which generally fails against a custom MinIO/RGW
  endpoint. Python sets no explicit style (boto3 uses its default `auto`
  resolution), so forcing path-style is a deliberate **choice**, not a proven
  equivalence; the MinIO parity test (below) validates it before it is relied
  on.

`aws-sdk-s3` exposes only a **client** (no boto3-style "resource" abstraction);
the port maps every boto3 resource call onto client operations. Types
(`cbscore-types`-free; owned here):

```rust
struct S3FileLocator { src: Utf8PathBuf, dst: String, name: String }
struct S3ObjectEntry { key: String, size: i64, last_modified: OffsetDateTime }
// .name() = basename of key (after the last '/')
struct S3ListResult { objects: Vec<S3ObjectEntry>, common_prefixes: Vec<String> }
```

Operations (each async, `Result<_, S3Error>`):

- `s3_upload_str_obj(secrets, url, bucket, key, body, content_type)` —
  `put_object` with a content type (default `application/json`). Thin wrappers
  `s3_upload_json` / `s3_download_json` fix the JSON content type.
- `s3_download_str_obj(secrets, url, bucket, key, content_type?) -> Option<String>`
  — `get_object`; a missing object returns **`None`**, not an error. The live
  not-found path is HTTP 404 on the `get_object` response (`s3.py:157-163`); the
  port maps a 404 response to `None` (the Python `NoSuchKey` catch at
  `s3.py:147` guards a no-IO handle and never fires). An optional content-type
  check errors on mismatch.
- `s3_upload_files(secrets, url, bucket, file_locs, public)` — upload a list of
  local files (`S3FileLocator`), optionally `ACL: public-read`. boto3's
  `upload_file` auto-multiparts; the port uses `put_object` with a file-backed
  `ByteStream` (single PUT). RPMs are well under the single-PUT ceiling, so
  multipart is **not required for correctness**; it is a noted follow-up only if
  artifacts ever approach the limit.
- `s3_list(secrets, url, bucket, prefix?, prefix_as_directory) -> S3ListResult`
  — `list_objects_v2` with **pagination** (continuation token loop, or the SDK
  paginator) and `Delimiter = "/"` when `prefix_as_directory`, collecting
  `CommonPrefixes` (logical subdirs) into `common_prefixes`.

`S3Error` is the subsystem's IO-layer error (lives here, per 002).

## Release S3 operations

Source: `releases/s3.py`. These compose the S3 client with the `ReleaseDesc` /
`ReleaseComponent` types (002). The on-disk S3 layout (under the releases
`bucket` + `bucket_loc`):

- **release descriptor**: `<bucket_loc>/<version>.json`
- **component release descriptor**: `<bucket_loc>/<name>/<version>.json`

Operations:

- `check_release_exists(secrets, url, bucket, loc, version) -> Option<ReleaseDesc>`
  — download `<loc>/<version>.json`; `None` if absent; parse error →
  `ReleaseError`. _(C6)_
- `release_desc_upload(secrets, url, bucket, loc, version, build) -> ReleaseDesc`
  — **read-modify-write**: load the existing release (or start an empty
  `ReleaseDesc { version, builds: {} }`), set `builds[build.arch] = build`,
  upload `<loc>/<version>.json`, and return the merged descriptor. (Non-atomic,
  matching Python; the build pipeline is the only writer per version.) _(C6)_
- `release_upload_components(secrets, url, bucket, loc, component_releases)` —
  upload each `<loc>/<name>/<version>.json` **concurrently**. _(C6)_
- `check_released_components(secrets, url, bucket, loc, components: map<name, version>) -> map<name, ReleaseComponent>`
  — download each `<loc>/<name>/<version>.json` **concurrently**; return those
  that exist (absent ones omitted; the caller decides whether to rebuild).
  _(C6)_
- `list_releases(secrets, url, bucket, loc) -> map<version, ReleaseDesc>` —
  `s3_list` the `<loc>/` prefix as a directory, then download+parse each
  `.json`; **malformed/old descriptors are skipped with a warning**, not fatal.
  _(C7)_

**Concurrency.** `release_upload_components` and `check_released_components` fan
out per component via `asyncio.TaskGroup` with `ExceptionGroup` aggregation
(`releases/s3.py:146-157, 212-224`). `TaskGroup` is **fail-fast with
cancellation**: the first task failure cancels every remaining sibling, and the
raised `ExceptionGroup` normally carries just that first error (the cancelled
siblings raise a suppressed `CancelledError`). The Rust port mirrors this with a
`JoinSet` that aborts the set on the first error (or `try_join_all`) — returning
the first `ReleaseError` and dropping the rest. It must **not** use `join_all`,
which runs every task to completion and would surface more errors than Python
ever does.

**The broken `list_releases` (port-discovered, not a 000-review finding).**
`list_releases` requires `(secrets, url, bucket, bucket_loc)`, but the CLI calls
it with only `(secrets, url)` (`cmds/versions.py:129`), so `versions list`
raises `TypeError` today. C7 **fixes** this: `bucket`/`loc` come from
`config.storage.s3.releases`, and `--from` supplies the host URL (see 006 /
010). The function body itself is correct and is ported as-is.

## Fidelity notes

- **Credentials from the secrets store**, injected explicitly — not the AWS
  default chain (H4).
- **Custom `endpoint_url`** (scheme-normalized) + **`force_path_style`** for
  MinIO/RGW; a fixed placeholder region (H4).
- **`get_object` 404 → `None`** (not an error), for both the download and the
  existence checks.
- **`list_objects_v2` pagination** (continuation token) and delimiter →
  `common_prefixes`.
- **Concurrent fan-out** for component upload/check via a `JoinSet` that aborts
  on the first error (fail-fast with cancellation), matching `TaskGroup` — never
  a run-all/collect-all `join_all`.
- **`list_releases` is lenient** — malformed descriptors are skipped, not fatal
  — and its CLI call site is fixed in C7 (M3).
- **No "resource" API** — `aws-sdk-s3` is client-only; boto3
  resource/`upload_file` calls map to `put_object`/`get_object`.

## Testing

- **MinIO parity** (the H4 gate): against a MinIO endpoint, exercise
  `put_object` / `get_object` / `list_objects_v2` with the secrets-injected
  credentials, custom `endpoint_url`, and `path-style` addressing; assert
  objects round-trip. This is the test that validates the path-style decision.
- **404 → `None`**: a download / existence check for a missing key returns
  `None`, not an error.
- **Pagination**: a listing spanning more than one page returns all objects;
  `common_prefixes` is populated for a delimited listing.
- **Release round-trip**: `release_desc_upload` then `check_release_exists`
  returns an equal `ReleaseDesc`; a second `release_desc_upload` for a different
  arch **merges** into the existing `builds` map rather than overwriting.
- **Concurrent fan-out**: `release_upload_components` /
  `check_released_components` over several components all succeed together; a
  failure in one cancels the rest and surfaces a single `ReleaseError`
  (fail-fast), matching `TaskGroup`.
- **Lenient list**: `list_releases` skips a malformed descriptor object and
  returns the valid ones.
