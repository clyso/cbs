# CRT — Design: patch annotations & richer `patch list` views

> **Status:** Design (seq `003`, drafted 2026-06-28, revised 2026-06-29
> addressing design reviews v1 and v2). Extends seq-001 (the v2 MVP — §3 domain
> model, §4 content addressing, §5 store) and builds on seq-002
> (`crt patch list` / `patch info`). Adds an operator-authored **annotations**
> layer on patches — applicability, tags, a description — kept **separate** from
> the git-derived `PatchMeta`, plus grouping and filtering for `patch list`.
> Authoritative; implementation deviations are recorded in `000-addendums.md`
> (none yet).

## 1. Motivation

Patches imported from a PR or a range are not interchangeable across releases. A
patch may be **generic** (applies to any ceph version, so any downstream
release) or a **version-specific backport** adapted to one ceph line. Because
backports for different lines are genuinely different code, content-addressing
already stores them as **distinct blobs** that often share the same `subject`
("literally the same name, different contents"). Today nothing records _which
ceph version a patch targets_, so an operator cannot:

- assess, at triage time, **which patches can be applied to a given downstream
  release** (which is itself based on a ceph version);
- tell two same-subject backports apart in `crt patch list`;
- categorize patches (`rgw`, `clyso/tunables`, …) or note what a patch does or
  when it is no longer needed.

This is **applicability** — a property used _before_ a release is curated. That
rules out deriving it from release manifests (seq-002 §"why not patch sets"): a
manifest tells you which release a patch _is in_, but triage needs which release
a patch _can go in_, which must exist before the manifest references the blob.

## 2. The split: immutable `PatchMeta` vs mutable `PatchAnnotations`

`PatchMeta` (seq-001 §3) holds facts **derived from the patch bytes** —
`blob_hash`, `patch_id`, `subject`, `body`, `cherry_picked_from` (a pure
function of `body`), `author`, `authored` — which are identical on every import
of the same blob. For those fields `put_meta`'s unconditional overwrite
(`crt/src/import.rs:269`) is therefore idempotent.

The remaining two `PatchMeta` fields are **not** blob-derived: `provenance` and
`source_repo` come from the import _invocation_ (a PR import records
`UpstreamPr{…}` and the remote slug; a range import records `Other{…}` and the
local path). Re-importing the same blob by a different route **overwrites these
last-importer-wins** — a pre-existing limitation of the content-addressed
overwrite, neither introduced nor fixed here. (A future `release add` guard that
leans on accurate `provenance` would need merge semantics for meta — out of
scope.) So the overwrite is idempotent for the blob-derived fields only, not
unconditionally.

The _new_ metadata is wholly different in kind: **operator-authored, mutable,
and accreting.** Folding it into `PatchMeta` would subject it to that same
overwrite and lose it on every re-import. So it lives in a **separate record per
blob**, written and merged independently of meta:

```
patches/meta/sha256/<blob_hash>.json          # PatchMeta — derived; overwritten on import
patches/annotations/sha256/<blob_hash>.json   # PatchAnnotations — operator-authored; merged
```

Import regenerates `meta` (overwrite) and **merges** into `annotations` (§5,
never clobbers). This keeps content-addressing as the trust anchor (seq-001 §1)
while giving mutable metadata a home that can grow. The annotations record is
**optional**: a blob without one reads as `None` (unassessed defaults), so
stores predating this design need no migration.

## 3. `PatchAnnotations` (`crt-core`)

```rust
// patches/annotations/sha256/<blob_hash>.json
struct PatchAnnotations {
    schema_version: u32,                  // = 1; operator-authored & not regenerable, so versioned
    applies_to: Option<Applicability>,    // None = not yet assessed
    tags: BTreeSet<String>,               // free-form: "rgw", "clyso/tunables"
    description: Option<String>,          // hard-typed: "what does this patch do?"
    attributes: BTreeMap<String, String>, // open bag: e.g. "retire-when" => "…"
}

#[serde(tag = "kind", content = "value", rename_all = "kebab-case")] // adjacent tagging
enum Applicability {
    Generic,                       // applies to any ceph version / release
    Versions(BTreeSet<VersionSpec>),
}

#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
enum VersionSpec {
    Line(String),   // major.minor, e.g. "18.2" — matches any v18.2.*
    Exact(String),  // a full version, v-prefixed, e.g. "v18.2.0" (or "v18.2.0-rc1")
}
```

- **`schema_version`** — `PatchAnnotations` is operator-authored and **not**
  regenerable (unlike `PatchMeta`, which carries no version because a re-import
  rebuilds it). Like `Manifest`, it is versioned so a future structural change
  (e.g. graduating an `attributes` key to a typed field) is a detectable
  migration, not a silent misparse.
- **Pinned serde — adjacent tagging.** `Applicability` and `VersionSpec` are
  persisted enums, so their wire format is fixed explicitly. Their unit/newtype
  variant shapes **preclude** the internal tagging `Provenance` uses
  (`#[serde(tag = "type")]` requires struct variants), so they use **adjacent**
  tagging (`tag` + `content`), valid for every variant kind — e.g.
  `{"kind":"line","value":"18.2"}`, `{"kind":"generic"}`. Variant renames cannot
  silently break stored records.
- **`applies_to`** — the one typed facet; drives version matching (§7) and a
  future `release add` guard. `None` = _unassessed_ (excluded from the version
  filter, never assumed generic). `Versions(∅)` is never stored — it normalizes
  to `None` (§5).
- **`tags`** — free-form categorization set (exact-match filterable).
- **`description`** — hard-typed, optional triage note ("what does this patch
  do?"), distinct from the upstream `subject`/`body` and a release's per-entry
  `public_summary`. Cleared with `--clear-description` (§5).
- **`attributes`** — open key→value bag for everything else (e.g. `retire-when`)
  so new facets need no schema churn; one that earns structure graduates to a
  typed field (bumping `schema_version`).

`crt-core` stays pure: it owns these types and the matching logic (§7) only; all
IO is in `crt-store`.

## 4. Store surface (`crt-store`)

Add to the `Store` trait, mirroring the meta methods:

```rust
async fn put_annotations(&self, hash: &Sha256, ann: &PatchAnnotations) -> Result<()>;
async fn get_annotations(&self, hash: &Sha256) -> Result<Option<PatchAnnotations>>; // None if absent
```

- Key: `patches/annotations/sha256/<blob_hash>.json`.
- `get_annotations` returns `Option` (absent ⇒ `None`, the unassessed default) —
  unlike `get_meta`, whose absence is an error, because a blob always has meta
  but need not have annotations. This mirrors the existing
  `get_patch_id → Option` pattern in `crt-store/src/lib.rs`.
- `put_annotations` is a whole-record write. **Merge is the caller's job** (read
  → merge → write, §5), so the store stays a dumb key/value layer. Listing
  reuses `list_patches` (seq-002) — annotations are keyed by the same blob
  hashes, so no new enumerator is needed.

## 5. Setting annotations

`applies_to` is **operator-asserted** (the tool never infers it, so it cannot
mis-claim applicability). It is set two ways, both **merging** into the existing
record (never clobbering tags / description / attributes).

**Version parsing (shared by setting here and querying in §7).** Strip an
optional leading `v`, then split off any trailing **pre-release tag** — the
suffix of the last `.`-component that begins with `-` (e.g. `-rc1`). Split the
remaining core on `.`; every part must be numeric, else it is a **hard error**.
Classify by the numeric-part count: **≥ 3 ⟹ `Exact`** (a point release, e.g.
`18.2.0`), stored v-prefixed with the tag re-attached (`v18.2.0`,
`v18.2.0-rc1`); **exactly 2 ⟹ `Line`** (major.minor, e.g. `18.2`), stored bare
(`18.2`); **< 2 ⟹ hard error**. A pre-release tag on a line input (`18.2-rc1`)
is a **hard error** (ambiguous, and never used by Ceph). The tag is kept only on
`Exact`, where it participates in exact-match and `canon()` (§7).

1. **Bulk, at import** — applied to _every_ patch of the PR/range:
   `--ceph-version <v>` **or** `--generic` (mutually exclusive — clap rejects
   both) plus repeatable `--tag <t>`. `description` is per-patch, so it is not a
   bulk flag (see `annotate`). A flag-less import touches no annotations.

2. **Per-patch** — `crt patch annotate <blob_hash> …`: `--ceph-version <v>` /
   `--unceph-version <v>` / `--generic` / `--unassessed`; `--tag <t>` /
   `--untag <t>`; `--description <text>` / `--clear-description`;
   `--set <k>=<v>` / `--unset <k>`. A single read-modify-write of the record.

**`applies_to` state transitions** (`X` is a parsed `VersionSpec`; `annotate`
reaches all rows, while import reaches only the `--ceph-version` / `--generic`
ones):

| Current       | Flag                 | Result                                           |
| ------------- | -------------------- | ------------------------------------------------ |
| `None`        | `--generic`          | `Generic`                                        |
| `None`        | `--ceph-version X`   | `Versions({X})`                                  |
| `None`        | `--unceph-version X` | `None` (no-op — nothing to remove)               |
| `Versions(S)` | `--ceph-version X`   | `Versions(S ∪ {X})`                              |
| `Versions(S)` | `--unceph-version X` | `Versions(S \ {X})`; `∅ ⟹ None`                  |
| `Versions(S)` | `--generic`          | `Generic` (absorbs)                              |
| `Generic`     | `--ceph-version X`   | `Generic` (absorbs; warns, no-op)                |
| `Generic`     | `--unceph-version X` | **error**: cannot exclude from `Generic` (below) |
| `Generic`     | `--generic`          | `Generic`                                        |
| any           | `--unassessed`       | `None`                                           |

`Generic --unceph-version X` **errors loudly** rather than silently leaving
`Generic` matching everything (which would violate §9's "no silent applicability
claims") — message: _"this patch is Generic; clear it with `--unassessed`, then
set specific `--ceph-version`s before excluding one."_ `None --unceph-version`
is, by contrast, a harmless **no-op** — `None` asserts nothing, so it can drop
nothing; no claim is made and no invariant is at stake. The asymmetry is
deliberate: the error fires only where a silent no-op would leave a false claim
standing.

Tag, description, and attribute flags are **orthogonal** to `applies_to` and
compose freely: `tags` union on `--tag` / subtract on `--untag`; `attributes`
set on `--set <k>=<v>` / remove on `--unset <k>`; `description` set on
`--description` / cleared on `--clear-description`.

The four `applies_to` operations — `--generic`, `--unassessed`, `--ceph-version`
(add; repeatable), `--unceph-version` (remove; repeatable) — are **mutually
exclusive within one invocation** (clap `conflicts_with`): a single call
performs at most one. So `--ceph-version A --ceph-version B` adds both, but
`--ceph-version A --unceph-version B`, `--generic --ceph-version`, and
`--unassessed --generic` are all rejected. They stay orthogonal to the tag /
description / attribute flags (e.g. `annotate <h> --unassessed --tag rgw` is
fine — applies_to reset, tag merged).

> **Future (out of scope):** the PR import path already fetches the PR
> `base_ref` (`import.rs:87-91`), but that is a branch _name_ (e.g. `reef`), not
> a version string — the §5 parser would reject it. Auto-suggesting `applies_to`
> would therefore need a branch→line map (`reef → 18.2`, …), not the version
> parser; the range path would need a heuristic on the range base. Deferred so
> the MVP stays operator-asserted.

## 6. Reading: `patch list` & `patch info`

**Grouping** (commit 1; needs no annotations):

```
crt patch list --group-by pr           # bucket by provenance.prs
crt patch list --group-by source-repo  # bucket by source_repo
```

`UpstreamPr{prs}` → one group per PR URL set (header: PR URL(s) + state);
`Other{description}` (local ranges) → a `(local range)` bucket keyed by the
description. Within a group, the seq-002 `(subject, blob_hash.to_hex())` sort; a
per-group `N patch(es)` summary on stderr. Once annotations exist, `--group-by`
also accepts `ceph-version` and `tag`. Under those, a patch matching several
values appears in **each** matching group (all-matching membership); `Generic`
patches form a `(generic)` group and unassessed (`None`) ones an `(unassessed)`
group. The version group key is the spec as stored — `18.2` for a `Line`,
`v18.2.0` for an `Exact`.

**Filtering** (annotations):

```
crt patch list --ceph-version v18.2.1   # Generic ∪ patches matching v18.2.1 (§7)
crt patch list --tag rgw                 # patches whose tags contain "rgw"
crt patch list --unassessed              # patches with applies_to = None (triage)
```

Filters compose with each other and with `--group-by`. The flat listing gains an
annotations-aware line so same-subject backports are distinguishable — e.g.
`<blob_hash>  <subject>  [18.2 | rgw]` (applicability + tags); exact text is a
plan-time detail. `patch info` shows the merged view (`PatchMeta` plus, when a
record exists, applicability/tags/description/attributes); its layout is also a
plan-time detail. (Filters scan via `list_patches` + per-blob `get_annotations`;
un-annotated blobs read as `None` — for `--unassessed` at scale, listing the
annotations prefix and diffing against meta is a future optimization, §10.)

**JSON output schema (and the seq-002 break).** seq-002 ships
`patch list --json` as a bare `[PatchMeta, …]`. seq-003 changes it; the shapes
are pinned here:

- **Element** (a "patch view"):
  `{ "meta": <PatchMeta>, "annotations": <PatchAnnotations> | null }`.
  `annotations` is JSON `null` when the blob has no record (explicit `null`, not
  an absent key).
- **Flat** `patch list --json` → `[ <element>, … ]`.
- **Grouped** `patch list --group-by <k> --json` →
  `[ { "group": <string>, "patches": [ <element>, … ] }, … ]`.

**Sequencing of the break.** Commit 1 adds only the grouped top-level shape
(under `--group-by`) and is **additive** — the flat `--json` stays
`[PatchMeta, …]`, so existing flat consumers are unaffected until annotations
land. Commit 2c introduces the `element` wrapper in **both** modes; that is the
single **breaking** change to the flat `--json`. (Commit 1's grouped shape also
gains the wrapper then — a second change on the grouped path, acceptable only
because `--json` is pre-stable.) seq-002's `--json` is treated as **pre-stable**
(no compatibility guarantee yet); a `--json-version` field is a future option
if/when it is promised stable.

## 7. Applicability matching

The `--ceph-version <q>` query (or a release `base_ref`, e.g. `v18.2.1`) is
parsed by the **same** rule as §5 — strip an optional `v`, split off any
`-prerelease` tag, then classify by the numeric-core part count (≥ 3 ⟹ a
**point** query; exactly 2 ⟹ a **line** query; otherwise a hard error). A
version's _line_ is its `major.minor`, ignoring any patch/sub-patch component
and any `-prerelease` tag (so the line of `v18.2.0`, `v18.2.1`, `v18.2.0-rc1`,
and `v18.2.0.1` is all `18.2`).

A patch matches iff its `applies_to` is:

- `Generic` → always; else
- `Versions(specs)` → **some** `spec` matches the query per this table; else
- `None` → never (unassessed; only `--unassessed` surfaces it).

| query \ spec        | `Line(L)`          | `Exact(E)`                          |
| ------------------- | ------------------ | ----------------------------------- |
| **point** `v18.2.1` | `line(query) == L` | `canon(query) == E` (full + suffix) |
| **line** `18.2`     | `query == L`       | `line(E) == query`                  |

where `line(x)` is `major.minor` and `canon(x)` is the v-prefixed full string
(suffix included). So `Exact("v18.2.0")` does **not** match the point query
`v18.2.0-rc1` (suffix differs), but both `v18.2.0` and `v18.2.0-rc1` fall in
`Line("18.2")`; the line query `18.2` matches `Exact("v18.2.1")` (same line).
Matching is pure (`crt-core`), table-driven, and **operator-asserted** — the MVP
does not trial-apply the patch (`git am`); the annotation is a curator's claim
for filtering and display.

## 8. Commit plan

Per the `git-commits` skill and the cbsd-rs CLAUDE.md granularity rule (split at
clean dependency boundaries), the work is six commits along the crate dependency
DAG — each compiles, is independently testable, and ships its own tests (no dead
seam):

- **Commit 1 — `crt`: group `patch list` by PR or source repo.**
  `--group-by pr|source-repo` + render + the **additive** grouped `--json` shape
  (flat `--json` unchanged, §6). `crt`-only; no store or `crt-core` change.
  Independent of everything below. (May declare the `ceph-version`/`tag`
  `--group-by` variants up front behind a "not yet available" error to avoid a
  later enum extension; deferring them to commit 4 is equally fine.)
- **Commit 2a — `crt-core`: `PatchAnnotations`, `Applicability`, `VersionSpec`,
  matching.** The pure types (with `schema_version` + adjacent-tagged serde) and
  the §5/§7 version parser + match logic, with **table-driven unit tests** over
  every §7 case (point/line, pre-release, sub-patch, v-prefix, and the
  `--unceph-version` round-trip). No IO, no CLI; the matching fn ships with its
  tests, so it is a tested library increment, not a dead seam.
- **Commit 2b — `crt-store`: `put_annotations` / `get_annotations`.** The
  `Store` trait extension + implementation + an `InMemory` round-trip test.
  Depends on 2a; no CLI.
- **Commit 2c — `crt`: annotate patches at import.** Import bulk flags
  (`--ceph-version`/`--generic`/`--tag`, mutual-exclusion, merge per §5), the
  `element`-wrapper `--json` break (§6), and `patch info` annotations rendering.
  The flat `patch list` **text** line is deliberately left unchanged here — it
  gains the annotations column in commit 4 alongside the filters; only `--json`
  and `patch info` surface annotations in 2c. Depends on 2b. Delivers: annotate
  at import + inspect.
- **Commit 3 — `crt`: edit patch annotations.** `crt patch annotate <hash>`
  (per-patch applicability incl. `--unceph-version`/`--unassessed` and the
  mutually-exclusive flag groups, tags, `description` incl.
  `--clear-description`, attributes; read-modify-write). Self-contained — kept
  separate, since folding it into the heavier 2c would re-create the oversized
  commit this split avoids.
- **Commit 4 — `crt`: filter and group `patch list` by applicability and tags.**
  `--ceph-version` / `--tag` / `--unassessed` filters, the annotations-aware
  list line, and `--group-by ceph-version|tag`. Delivers: triage views.

Each gates with `cargo fmt` + `clippy --workspace --all-targets` + `cargo test`.

## 9. Invariants (do not regress)

- **`PatchMeta` overwrite semantics are untouched.** Blob-derived fields are
  stable across imports; `provenance`/`source_repo` remain last-importer-wins
  (§2) — this design neither fixes nor worsens that.
- **Import never clobbers annotations.** Bulk flags merge per §5; a flag-less
  re-import leaves the annotations record untouched.
- **Content-addressing preserved.** No new identity; annotations are keyed by
  the existing `blob_hash`; reuse across releases is unaffected.
- **`crt-core` stays pure** (types + matching only); IO is `crt-store`; CLI is
  `crt`. Result on stdout, diagnostics on stderr (seq-002).
- **No silent applicability claims.** `applies_to = None` is excluded from the
  version filter — never treated as `Generic`; and `Generic --unceph-version`
  errors rather than silently no-op'ing (§5).
- **Single-writer assumption.** Import and `annotate` do a non-transactional
  read-modify-write of the annotations record (last-writer-wins, no CAS),
  matching `import_commit`'s own non-transactional note and seq-001 §5's
  future-service seam. Multi-writer safety is out of scope.

## 10. Out of scope (future)

- **`release add` applicability guard** — reject (or warn on) adding a patch
  whose `applies_to` excludes the draft's `base_ref`. The metadata here enables
  it; the guard is separate work.
- **Manifest-derived "used by" view** — "which sealed releases reference this
  blob" is derivable from manifests and complements annotations in `patch info`;
  not the applicability mechanism, and not built here.
- **Auto-derived applicability** from the import base (§5).
- **Structured lifecycle** — `retire-when`/`superseded-by` graduating from
  `attributes` to typed fields with their own checks.
- **A faster filter path / labels index** — the per-blob record + `list_patches`
  scan (with absent reads for un-annotated blobs) is sufficient at CLI scale; an
  annotations-prefix listing diffed against meta, or a
  `patches/labels/<k>/<v>/<hash>` index, is a later optimization only if
  enumeration cost demands it.
- **Merge semantics for `PatchMeta.provenance`/`source_repo`** so re-import
  unions rather than overwrites lineage (§2).
