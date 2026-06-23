# Review — 005 Storage (S3) & releases (v1)

Adversarial design review of
`design/005-20260621T1813-storage-s3-and-releases.md`. Every claim was checked
against the real Python in `cbscore/utils/s3.py`, `cbscore/releases/s3.py`, and
`cbscore/releases/desc.py`; the cross-design references were checked against 002
and 004 and the `versions` CLI call site. Settled decisions (H4 resolution, the
C7 call-site fix, 002-owned types, IO-layer error placement) were not
relitigated — only checked for honest representation.

## Verdict

**go-with-changes.**

The design is faithful on the bulk of the surface: the explicit-credentials
injection, the scheme normalization, the placeholder region, the S3 key layout,
the read-modify-write merge, the 404-to-`None` effect, the pagination loop and
delimiter / common-prefixes handling, the lenient `list_releases`, and the M3
broken-call-site diagnosis are all accurate against the source. The single
blocking defect is the **concurrency characterization**: the design describes
`asyncio.TaskGroup` as run-all / collect-all-errors / "other results still
attempted," which is the inverse of `TaskGroup`'s actual
fail-fast-with-sibling-cancellation behavior, and it offers `JoinSet` and
`futures::future::join_all` as interchangeable when they have opposite
cancellation semantics. An implementer following the doc literally builds the
wrong concurrency contract. This is a localized clarification, not a structural
flaw, so it caps the verdict at go-with-changes rather than no-go.

## Confidence

| Item                                                                 | Points | Description                                                                                         |
| -------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------- |
| Starting score                                                       | 100    |                                                                                                     |
| D8: concurrency semantics inverted (`TaskGroup` is fail-fast/cancel) | -5     | Design claims collect-all + "others still attempted"; `TaskGroup` cancels siblings on first failure |
| D8: `JoinSet` vs `join_all` offered as equivalent                    | -5     | Opposite cancellation behavior; `join_all` never cancels, so it is _less_ faithful, not equal       |
| D10: line 39 overclaims path-style "matches effective behavior"      | -5     | boto3 default addressing is `auto` (not path-style); equivalence is unproven until the parity test  |
| D11: 404/`NoSuchKey` mapping point left implicit                     | -5     | Design does not note the live 404 path is on `get_object`, not a dead `NoSuchKey` catch             |
| **Total**                                                            | **80** |                                                                                                     |

Interpretation: 80 — acceptable with the noted corrections; fix the concurrency
wording before this design drives implementation.

## Findings (by severity)

### F1 — Concurrency semantics are inverted (BLOCKING; required change)

**Design claim.** Lines 105–110: the fan-out uses `asyncio.TaskGroup` with
`ExceptionGroup` aggregation, and the Rust port "fans out with a `JoinSet` (or
`futures::future::join_all`); **all** per-component errors are collected (not
just the first), and a combined `ReleaseError` is returned — preserving Python's
aggregate-and-report behavior rather than failing fast on the first." Reinforced
at line 130 ("aggregated errors … matching `TaskGroup`/`ExceptionGroup`") and
line 152 ("a failure in one is surfaced as an aggregated `ReleaseError` (other
results still attempted)").

**What the Python actually shows.** `releases/s3.py:146-157` and
`releases/s3.py:213-224` use `asyncio.TaskGroup`. `TaskGroup`'s contract is
fail-fast with sibling cancellation:

- On the first task exception, the group aborts and **cancels every other
  not-yet-done task**.
- The resulting `ExceptionGroup` is built only from tasks that raised on their
  own; tasks cancelled by the abort raise `CancelledError`, which the group
  **suppresses** — it is not added to the group.
- So in the normal case the `ExceptionGroup` carries **only the first failure**,
  and the siblings are cancelled, not completed. Multiple entries appear only if
  several tasks happen to fail within the same event-loop step before the abort
  propagates.

The Python's own handler is consistent with this: `e.subgroup(ReleaseError)`
(s3.py:153, 220) iterates whatever failures landed in the group, but the group
does **not** wait for or report cancelled siblings.

**The gap.** Three statements are wrong or misleading:

1. "**all** per-component errors are collected (not just the first)" (line 109)
   is typically false — usually only the first failing task's error is in the
   group; the rest are cancelled.
2. "preserving Python's aggregate-and-report behavior rather than failing fast"
   (line 110) and "other results still attempted" (line 152) **invert** reality:
   `TaskGroup` _does_ fail fast and _does_ cancel siblings.
3. `JoinSet` and `futures::future::join_all` are offered as interchangeable
   (line 107), but `join_all` runs every future to completion and never cancels
   — a `join_all` port would surface **more** errors than the Python ever would,
   which is the opposite of fidelity. `JoinSet` (with `abort_all` / drop on
   first error) or `try_join_all` is the cancellation-faithful mapping.

**Recommended change.** The design must pick one model and label it honestly:

- (a) **Faithful port** — `try_join_all`, or a `JoinSet` that aborts the
  remaining tasks on the first error; the returned `ReleaseError` is built from
  the task(s) that actually failed before the abort. This matches `TaskGroup`
  and should be described as fail-fast with sibling cancellation, not
  collect-all.
- (b) **Deliberate run-all-collect-all** (`join_all`) — defensible because the
  per-component operations are idempotent (independent `put_object` /
  `get_object` per key) and a complete error list is arguably more useful, but
  it is an **intentional deviation** from the Python and must be documented as
  such, not as "preserving Python's behavior."

Either is acceptable; the doc must stop describing fail-fast as collect-all. The
testing bullet at line 151–152 ("other results still attempted") must be
reworded to match whichever model is chosen.

### F2 — Path-style "matches effective behavior" is an overclaim (medium)

**Design claim.** Lines 36–40: set `force_path_style(true)`; "Python sets no
explicit style, so the port matches the deployment's effective behavior —
path-style is the expected requirement and is validated by a MinIO parity test."

**What the Python actually shows.** `s3.py:100, 143, 274` construct the session
and resource/client with no addressing-style argument. The resource calls are
positional: `s3_session.resource("s3", None, None, True, True, hostname)` — i.e.
`service_name="s3"`, `region_name=None`, `api_version=None`, `use_ssl=True`,
`verify=True`, `endpoint_url=hostname`. No `s3={"addressing_style": …}` config
is passed anywhere, so boto3 falls to its default resolution (`auto`), which is
**not unconditionally path-style** — it depends on bucket-name DNS-compatibility
and endpoint. The list path uses `client("s3", endpoint_url=hostname)`
(s3.py:330), again with no explicit style.

**The gap.** Forcing path-style in the port is a behavioral _change_ from
boto3's `auto`, not a match of "the deployment's effective behavior." The two
may coincide in practice for MinIO/RGW, but that equivalence is exactly what is
unproven until the parity test runs. The decision (force path-style, validate
with MinIO) is sound and sanctioned; only the wording at line 39 overstates the
current equivalence.

**Recommended change.** Soften line 39 to state that boto3 uses default (`auto`)
addressing, that forcing path-style is a deliberate change expected to be
correct for MinIO/RGW, and that the parity test is the gate that confirms it.
The positional `resource(...)` arg mapping above is worth recording in the
design so the "no explicit style" claim is grounded.

### F3 — 404 / `NoSuchKey` mapping point is left implicit (minor)

**Design claim.** Lines 58–61, 124–126: `get_object` with a missing object
(`NoSuchKey` / HTTP 404) returns `None`.

**What the Python actually shows.** In `s3_download_str_obj` the
`except s3.meta.client.exceptions.NoSuchKey` at `s3.py:147` guards
`bucket.Object(location)` — but `Object()` constructs a resource handle and
performs no network IO, so that catch is effectively dead. The live
404-to-`None` path is the `ClientError` branch checking `HTTPStatusCode == 404`
at `s3.py:157-163`, reached when `obj.content_type` (and then `obj.get()`)
actually hit the network.

**The gap.** The effect-level claim ("404 → `None`") is correct, but the design
does not say _where_ the 404 is observed. In `aws-sdk-s3` there is no lazy
`Object()` handle; the port must map the not-found condition on the actual
`get_object` call (`GetObjectError::NoSuchKey`, or the HTTP-404 service error).
Without this note an implementer may look for a `NoSuchKey` on a non-existent
"get handle" step.

**Recommended change.** Add a sentence: the 404/`NoSuchKey` mapping applies to
the `get_object` response; there is no separate object-handle step in the Rust
client.

## Confirmed faithful (no action)

These were checked against source and are accurate; listed so the author knows
they were verified, not skipped:

- **Explicit credentials, not the default chain.**
  `s3.py:92-95, 135-138, 266-269` build the session with `aws_access_key_id` /
  `aws_secret_access_key`; the design's static-provider mapping is correct.
  `secrets.s3_creds(url)` returns `(hostname, access_id, secret_id)`
  (`secrets/mgr.py:74`), matching 004 and the design's tuple.
- **Scheme normalization.** `if not hostname.startswith("http"):` → `https://`
  prefix (`s3.py:97-98, 140-141, 271-272, 318-319`). Design line 32 is accurate.
  (Minor: Python tests `startswith("http")`, which also accepts an existing
  `http://`; immaterial to the design.)
- **Placeholder region.** Python passes `region_name=None`; the SDK requires a
  region. Design lines 33–35 are correct.
- **S3 key layout.** Release: `{bucket_loc}/{version}.json`
  (`releases/s3.py:42, 95`). Component: `{bucket_loc}/{name}/{version}.json`
  (`releases/s3.py:131, 187`). Design lines 81–82 match exactly.
- **Read-modify-write merge.** `release_desc_upload` loads the existing
  descriptor (or `ReleaseDesc(version, builds={})`), sets
  `builds[release_build.arch]`, uploads, returns the merged value
  (`releases/s3.py:91-113`). Non-atomic, single-writer-per-version. Design lines
  89–93 are faithful, including the round-trip merge test.
- **Lenient `list_releases`.** Malformed JSON is logged and `continue`d, not
  fatal (`releases/s3.py:276-279`); non-`.json` entries skipped
  (`releases/s3.py:252-254`). Design lines 100–103, 131–132, 153–154 match.
- **M3 broken call site.** `cmds/versions.py:129` calls
  `list_releases(secrets, s3_address_url)` — 2 args — while the function
  requires `(secrets, url, bucket, bucket_loc)`, so `versions list` raises
  `TypeError` today. The design correctly attributes the fix to C7 and ports the
  function body as-is (lines 112–117).
- **Pagination + delimiter.** `list_objects_v2` continuation-token loop with
  `IsTruncated` / `NextContinuationToken`, `Delimiter="/"` when
  `prefix_as_directory`, and `CommonPrefixes` collected into a set
  (`s3.py:334-376`). Design lines 68–71, 127–128 match. (The Python also forces
  `prefix_as_directory = False` when `prefix` is empty, `s3.py:324-325` — a
  small detail the port should keep.)
- **`upload_file` → `put_object`, single-PUT.** `_upload_file` uses
  `bucket.upload_file(..., ExtraArgs={"ACL": "public-read"})` when public
  (`s3.py:228, 234-238`). Dropping to a single `put_object` with a file-backed
  `ByteStream` is sound for RPM-sized artifacts (well under the 5 GB single-PUT
  ceiling); the multipart follow-up note is appropriate. No content-type
  regression exists here: boto3 `upload_file` does not auto-detect content type
  either, so neither path sets one.
- **Type fidelity.** `S3ObjectEntry { key, size, last_modified }` with a `.name`
  basename property (`s3.py:45-57`), `S3ListResult { objects, common_prefixes }`
  (`s3.py:60-64`), `S3FileLocator { src, dst, name }` (`s3.py:34-42`). Design
  lines 47–50 match; `size: i64` is a reasonable Rust mapping of the Python
  `int`.
- **Error placement.** `S3Error` and `ReleaseError` are IO-layer errors that
  live with this subsystem, consistent with 002 (design 002 lines 237–248).
