# 002 — Wire types, errors & schema versioning

This is the reference design for the `cbscore-types` crate's central surface:
the wire types cbscore **produces**, the schema-versioning machinery they share,
and the type-layer error taxonomy. It is the single source of truth for those
types; every other subsystem design references them rather than redefining them.
Read 001 first for the crate layout and the pragmatic bump policy this document
implements.

This crate is **zero-IO**: it defines types and pure helpers only — no file IO,
no subprocess, no async, no cloud SDK, no `regex`. It may depend on `serde`,
`serde_json`, `thiserror`, `camino`, and `uuid`.

## Scope: what lives here vs. elsewhere

Defined here (the cross-cutting, cbscore-**produced** formats and shared
machinery):

- `VersionDescriptor` and its sub-types.
- `ReleaseDesc` and its sub-types.
- `BuildArtifactReport` and its sub-types.
- The `schema_version` machinery and casing conventions.
- The `VersionType` enum.
- The type-layer error taxonomy.
- Tracing-target constants.

Defined in their consuming subsystem designs, **not** duplicated here:

- **Config and secrets types** → 004 (they live in `cbscore-types` too, and use
  the `schema_version` machinery specified here, but their field shapes belong
  with the config/secrets/Vault design).
- **`ContainerDescriptor`** (external component-repo input, with a discriminated
  `repos` union and `{version}`/`{el}` template interpolation) → 008.
- **`ImageDescriptor`** (external repo input, resolved by `get_image_desc`)
  → 006.

## Schema-versioning machinery

Per 001, only the formats cbscore **produces or owns** carry a version marker.
This document covers the three produced descriptors/report; config and secrets
(004) reuse the same machinery.

- **`VersionDescriptor`, `ReleaseDesc`** carry a `schema_version: u32`.
- **`BuildArtifactReport`** keeps its existing marker name,
  `report_version: u32` (converging on `schema_version` is a roadmap item; the
  machinery below is identical, only the field name differs).
- **Container/image descriptors are not versioned** (external inputs).

Mechanics, identical for every marker:

```rust
fn schema_v1() -> u32 { 1 }

#[serde(default = "schema_v1")]
schema_version: u32,   // (report_version on BuildArtifactReport)
```

- **Absent → v1.** `#[serde(default)]` makes a marker-less file (every file
  written before this port) parse as v1. Existing version/release descriptors
  keep working unchanged.
- **Always written.** Rust always serializes the marker, so Rust-produced files
  carry it explicitly.
- **Unknown is a hard error, not a silent mis-parse.** A parser accepts a marker
  `<=` the maximum version it implements; a higher value is rejected with a
  typed error (see the error taxonomy). Per the pragmatic bump policy (001), a
  higher marker can only mean a breaking change the parser does not understand.
- **Bump rule (from 001):** additive backward-compatible fields (optional,
  defaulted, skipped-if-absent) do **not** bump; rename/remove/retype or a
  semantic shift does.

All current produced formats are at **v1**.

## Casing conventions (hard invariant)

The Rust serde representation must reproduce the existing on-disk key names
exactly.

- **Descriptors, releases, report** — plain `snake_case`. No `rename_all`. The
  version marker keys are `schema_version` (snake) and `report_version` (snake).
- **Config, secrets** (004) — `#[serde(rename_all = "kebab-case")]`. Their
  marker key is therefore `schema-version` (kebab).
- Rust keyword fields are renamed back to the wire name, e.g. the component
  `ref` field uses `#[serde(rename = "ref")]`.

## `VersionDescriptor` (produced; JSON; snake_case)

Source of truth: `cbscore/versions/desc.py`. Written by `versions create` to
`<store>/<type>/<VERSION>.json` via `model_dump_json(indent=2)`; read by the
runner and the builder.

```rust
struct VersionDescriptor {
    schema_version: u32,            // NEW marker; absent → 1
    version: String,
    title: String,
    signed_off_by: VersionSignedOffBy,
    image: VersionImage,
    components: Vec<VersionComponent>,
    distro: String,
    el_version: u32,
}

struct VersionSignedOffBy { user: String, email: String }
struct VersionImage { registry: String, name: String, tag: String }
struct VersionComponent {
    name: String,
    repo: String,
    #[serde(rename = "ref")]
    git_ref: String,
}
```

The Rust on-disk output equals Python's shape **plus** the `schema_version`
marker; a golden-file test asserts the field names and nesting match Python's
`model_dump_json` output (accounting for the added marker). The `image` block
here is the version descriptor's own `registry`/`name`/`tag`, distinct from the
external `ImageDescriptor` (006).

## `ReleaseDesc` (produced; JSON; snake_case)

Source of truth: `cbscore/releases/desc.py`. Uploaded to S3 by the builder; read
by the builder (reuse checks) and `versions list`.

The Python types use **mixin inheritance**
(`ReleaseComponentVersion(ReleaseComponentHeader, BuildInfo)`), which produces a
**flat** JSON object. The Rust port flattens the mixins into explicit fields (or
`#[serde(flatten)]` of embedded structs) so the JSON shape is byte-for-byte the
same.

```rust
enum ArchType {                     // serde rename "x86_64"
    #[serde(rename = "x86_64")] X86_64,
}
enum BuildType {                    // serde rename "rpm"
    #[serde(rename = "rpm")] Rpm,
}

struct ReleaseDesc {
    schema_version: u32,            // NEW marker; absent → 1
    version: String,
    builds: BTreeMap<ArchType, ReleaseBuildEntry>,
}

struct ReleaseBuildEntry {          // = BuildInfo + components
    arch: ArchType,
    build_type: BuildType,
    os_version: String,             // e.g. "el9"
    components: BTreeMap<String, ReleaseComponentVersion>,
}

struct ReleaseComponentVersion {    // = BuildInfo + header + extras (flat)
    // pydantic v2 collects inherited fields in reverse-MRO order, so the
    // BuildInfo fields serialize FIRST, then the header, then the extras.
    arch: ArchType,
    build_type: BuildType,
    os_version: String,
    name: String,
    version: String,
    sha1: String,
    repo_url: String,
    artifacts: ReleaseRpmArtifacts,
}

struct ReleaseComponent {           // = header + versions
    name: String,
    version: String,
    sha1: String,
    versions: Vec<ReleaseComponentVersion>,
}

struct ReleaseRpmArtifacts { loc: String, release_rpm_loc: String }
```

`ArchType` and `BuildType` are single-variant today (`x86_64`, `rpm`); they stay
enums so the wire format can grow without a retype. `builds` and `components`
use `BTreeMap`: deterministic (sorted) key ordering matters for stable output,
while Python's dict insertion order does not (map entries are parsed by key, and
byte-equality with Python is a non-goal — 001). `ReleaseComponent` (the
per-component release descriptor uploaded separately) is included here because
it shares the header/version shapes; its S3 layout is specified in 005.

## `BuildArtifactReport` (produced; JSON; snake_case; marker `report_version`)

Source of truth: `cbscore/builder/report.py`. Written in-container to the
scratch mount; read on the host by the runner (before the return-code check —
invariant 5 in 001); propagated to the worker and server.

```rust
struct BuildArtifactReport {
    report_version: u32,            // KEEP this name (see 001 / ROADMAP)
    version: String,
    skipped: bool,
    container_image: Option<ContainerImageReport>,
    release_descriptor: Option<ReleaseDescriptorReport>,
    #[serde(default)]
    components: Vec<ComponentReport>,
}

struct ContainerImageReport { name: String, tag: String, pushed: bool }
struct ReleaseDescriptorReport { s3_path: String, bucket: String }
struct ComponentReport {
    name: String,
    version: String,
    sha1: String,
    repo_url: String,
    rpms_s3_path: Option<String>,   // emit `null` when unset (matches Python)
}
```

`container_image` is populated for both the skipped and full-build paths;
`release_descriptor` and `components` are empty/`None` on the skipped path.
Optionality follows two consistent rules that match pydantic's `model_dump_json`
output: every `Option` field serializes as `null` when unset (no
`skip_serializing_if`; serde also deserializes an absent key to `None`), and
`components: Vec` uses `#[serde(default)]` so it tolerates an absent key and
always serializes as `[]`. The worker and `cbsd-server` already consume this
format with the `report_version` field name; keeping it avoids a cross-component
rename now (the convergence is roadmapped).

## `VersionType`

Source of truth: `cbscore/versions/utils.py`. The enum and its serde
representation live here; the **lookup helper** (`get_version_type`, a name→type
table lookup — not a `regex` parser), the per-type CLI labels, and the human
descriptions used for titles are version-domain data and are specified in 006
(versions), where they are consumed.

```rust
#[serde(rename_all = "lowercase")]   // Python StrEnum values: release/dev/test/ci
enum VersionType { Release, Dev, Test, Ci }
```

## Error taxonomy (type-layer)

`cbscore-types` holds the **type-layer** errors — parse/validation failures for
the wire types and version-string errors. These are pure (`thiserror`) enums
with no IO. Subsystem error enums that arise from IO or subprocess work (git,
podman, buildah, skopeo, S3, Vault, secrets) are defined **with their
subsystems** in cbscore (003, 004, 005, …), to keep each subsystem's errors as
its single source of truth. This refines 001's "error taxonomy in
`cbscore-types`" to: the type layer's errors live here; the IO layers' errors
live with their subsystems.

The split is by what **triggers** the error, not merely what it describes: pure
parse/validation and version-string errors live in `cbscore-types`; "not
found"/IO-triggered errors (a descriptor file that is absent, no image
descriptor matching a version) are raised by the subsystem that performs the IO
and are defined there.

Type-layer errors (in `cbscore-types`; names mirror the Python originals):

- `MalformedVersion` — a version string that does not match the accepted shapes
  (analogue of `MalformedVersionError`).
- `VersionError` — version-domain failures (unknown version type, etc.).
- `InvalidVersionDescriptor { path }` — a descriptor whose bytes fail to parse
  or validate (analogue of `InvalidVersionDescriptorError`). The `path` is
  carried for context; the type itself performs no IO.
- `UnknownSchemaVersion { format, found, max }` — a marker higher than the
  parser implements. This is **not** produced by a serde attribute: after
  deserialization (which applies `absent → v1`), a validation step compares the
  marker against the maximum the parser implements and returns this error if it
  is higher.

IO-triggered "not found" errors live with their subsystems, not here:
`NoSuchVersionDescriptor` (the descriptor file is absent — raised by the reader
in the runner/versions subsystem, analogue of `NoSuchVersionDescriptorError`)
and `NoSuchVersion` (no image descriptor matched — raised by `get_image_desc`,
analogue of `NoSuchVersionError`, specified in 006).

Conventions: every public error implements `std::error::Error` via `thiserror`;
subsystem errors wrap lower-level errors with `#[from]` / `#[source]` and never
expose a boxed `dyn Error` in their public API. `anyhow` is used only at
`cbsbuild`'s `main` boundary (001).

## Tracing targets

`cbscore-types` defines the tracing-target constant hierarchy as plain string
constants (no global-state mutation), e.g. `cbscore::utils::subprocess`,
`cbscore::utils::git`, `cbscore::runner`, `cbscore::builder`. The subscriber
setup and the `CBS_DEBUG` → level mapping live in `cbsbuild` (010); the
constants live here so every subsystem references one source.

## Round-trip fidelity & testing

- **Round-trip stability** (invariant 1): for each produced type,
  `serialize → parse → equal`.
- **Golden-file parity**: assert the produced JSON matches Python's
  `model_dump_json` output for `VersionDescriptor` and `ReleaseDesc` by field
  name, nesting, and value — compared as parsed values, not byte-for-byte (key
  order is not significant; byte-equality is a non-goal per 001), and accounting
  for the added `schema_version` marker. Operators read these files, so the
  field set and shape must match.
- **Marker handling**: tests that a marker-less file parses as v1, that a v1
  file round-trips with the marker present, and that a higher marker is rejected
  with `UnknownSchemaVersion`.
- **Mixin flattening**: a test pinning that `ReleaseComponentVersion` serializes
  as a flat object with fields in pydantic's reverse-MRO order — the `BuildInfo`
  fields first (`arch, build_type, os_version`), then the header
  (`name, version, sha1`), then `repo_url, artifacts`.
