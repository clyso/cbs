# Adversarial design review — wire types, errors & schema versioning (002)

- **Type:** design review (adversarial, design-level)
- **Reviews:** design seq 002,
  `design/002-20260621T1055-wire-types-errors-and-schema-versioning.md`
- **Date:** 2026-06-21
- **Verdict:** GO WITH CHANGES. The schema-versioning machinery, casing
  invariant, error analogues, and `VersionType` variants are all sound and match
  the Python source. But one **verified wire-fidelity defect** (the
  `ReleaseComponentVersion` flattened field order is reversed) directly
  contradicts 002's own golden-file-parity claim, and a cluster of
  serialize-shape inconsistencies in `BuildArtifactReport` would produce JSON
  that disagrees with the Python producer this type still interoperates with.
  None breaks the type architecture; all should be folded in before the
  `cbscore-types` plan derives struct definitions from this document.
- **Confidence:** 74 / 100 (see table).

## What is being reviewed

Design 002 is the single source of truth for the `cbscore-types` crate's central
surface: the three wire types cbscore **produces** (`VersionDescriptor`,
`ReleaseDesc`, `BuildArtifactReport`), the shared `schema_version` machinery,
the casing invariant, the `VersionType` enum, the type-layer error taxonomy, and
the tracing-target constants. It is zero-IO and declares itself the reference
every other subsystem design points at rather than redefining.

## Method

Every type, field, optionality, and serialized form named in 002 was checked
against the authoritative Python pydantic models — `versions/desc.py`,
`releases/desc.py`, `builder/report.py`, `versions/utils.py` — and the error
analogues against `errors.py`, `versions/errors.py`, `images/errors.py`. Where
pydantic's serialized shape is subtle (multiple-inheritance field ordering, the
treatment of unset `Optional` fields, enum value casing) the behavior was not
reasoned about — it was **executed** against pydantic 2.13.4 with the real
models, because the mandate is to verify each field, never assume. The empirical
commands and their output are cited inline.

The findings are judged against the parity bar 002 actually sets, not a stricter
one. 001 makes cross-language byte-equality a **non-goal** (001:29-32:
"Cross-language byte-equality of wire formats … round-trip stability within Rust
suffices. No steady-state file interchange between Python and Rust"). 002,
however, makes a stronger **structural** claim: the serde representation "must
reproduce the existing on-disk key names exactly" (002:74-76) and golden-file
tests "assert the produced JSON's field names and nesting match Python's
`model_dump_json` output" (002:264-271). Field names and nesting — not
byte-equality — are therefore the in-scope bar. Findings below are scored
against that bar.

## Confidence score

Starting from 100, each distinct defect deducts by severity. The
`confidence-scoring` criteria (D1–D12) are code-review triggers; for a design
document they are mapped onto severity bands, mirroring review 001's adaptation
for consistency within this doc set.

| Deduction      | Pts    | Finding                                                               |
| -------------- | ------ | --------------------------------------------------------------------- |
| Starting score | 100    |                                                                       |
| High H1        | −10    | `ReleaseComponentVersion` flat field order is reversed vs. pydantic   |
| Med M1         | −5     | `rpms_s3_path` `skip_serializing_if` omits a key Python emits as null |
| Med M2         | −4     | Report optionality is internally inconsistent (3 fields, 3 rules)     |
| Med M3         | −4     | `VersionType` serde repr is PascalCase; Python `StrEnum` is lowercase |
| Med M4         | −3     | `NoSuchDescriptor`/`NoSuchVersion` are IO errors in the zero-IO crate |
| Low L1         | −2     | "unknown = hard error" is not expressible by the shown serde attr     |
| Low L2         | −2     | `BTreeMap` re-sorts keys; Python dict preserves insertion order       |
| Low L3         | −1     | `InvalidDescriptor`/`NoSuchDescriptor` drop the `Version` name token  |
| **Total**      | **74** | Fold H1/M1–M4 before the crate's structs are written                  |

## Findings

### High H1 — the `ReleaseComponentVersion` flattened field order is reversed vs. the actual pydantic output

**Design claim.** 002:151-160 declares the flattened struct header-first:

```
struct ReleaseComponentVersion {    // = header + BuildInfo + extras (flat)
    name: String, version: String, sha1: String,   // header first
    arch: ArchType, build_type: BuildType, os_version: String,  // then BuildInfo
    repo_url: String, artifacts: ReleaseRpmArtifacts,
}
```

and 002:264-277 promises golden-file parity ("field names and nesting match
Python's `model_dump_json` output") plus a dedicated test "pinning that
`ReleaseComponentVersion` serializes as a flat object … matching the Python
inheritance output."

**What the Python code actually shows.** The class is
`ReleaseComponentVersion(ReleaseComponentHeader, BuildInfo)`
(`releases/desc.py:64`). `ReleaseComponentHeader` declares `name, version, sha1`
(`desc.py:43-48`); `BuildInfo` declares `arch, build_type, os_version`
(`desc.py:35-40`). Pydantic v2 collects inherited fields in **reverse MRO**
(base-most first), so `BuildInfo`'s fields precede the header's. Executed
against the real model:

```
$ PYTHONPATH=cbscore/src python -c \
  "from cbscore.releases.desc import ReleaseComponentVersion as R; \
   print(list(R.model_fields))"
['arch', 'build_type', 'os_version', 'name', 'version', 'sha1',
 'repo_url', 'artifacts']
```

The real serialized order is **BuildInfo-first**:
`arch, build_type, os_version, name, version, sha1, repo_url, artifacts`. The
design's struct is the exact opposite for the first six fields.

**The gap.** This is a verified field-name/ordering divergence — the very thing
002's golden-file-parity claim (002:264-271) and its mixin-flattening test
(002:274-277) exist to catch. An implementer who transcribes 002's struct
literally produces JSON whose keys appear in a different order than the Python
producer's. Whether that breaks a consumer depends on the consumer, but it
falsifies the design's own parity assertion, and the "matching the Python
inheritance output" test it prescribes would **fail** against the struct as
drawn. Note that single-inheritance siblings are fine: `ReleaseBuildEntry`
(`['arch','build_type','os_version','components']`) and `ReleaseComponent`
(`['name','version','sha1','versions']`) match the design — only the
double-inheritance type is reversed.

**Recommended change.** Reorder the `ReleaseComponentVersion` struct to
`arch, build_type, os_version, name, version, sha1, repo_url, artifacts` to
match pydantic's reverse-MRO output, OR (cleaner and self-documenting) define it
with two `#[serde(flatten)]` embedded structs in the order `BuildInfo` then
`ReleaseComponentHeader` then the extras, since `#[serde(flatten)]` emits fields
in the flattened structs' declared order. Either way, state explicitly in the
doc that the flattened order is BuildInfo-first, so the prescribed golden test
pins the correct order rather than the inverted one.

### Medium M1 — `rpms_s3_path`'s `skip_serializing_if` omits a key the Python producer emits as `null`

**Design claim.** 002:198-205 gives `ComponentReport.rpms_s3_path` the attribute
`#[serde(skip_serializing_if = "Option::is_none", default)]`, so an unset value
produces **no key** in the JSON.

**What the Python code actually shows.** `rpms_s3_path: str | None = None`
(`builder/report.py:74`) carries no exclusion config, and no model sets
`model_config` (verified: `dict(ComponentReport.model_config) == {}`). Pydantic
v2's default `model_dump_json` **emits** unset optionals as `null`. Executed:

```
$ ... python -c "from cbscore.builder.report import ComponentReport as CR; \
  print(CR(name='ceph', version='19.2.3', sha1='abc', \
           repo_url='https://x').model_dump_json(indent=2))"
{ "name": "ceph", "version": "19.2.3", "sha1": "abc",
  "repo_url": "https://x", "rpms_s3_path": null }
```

The Python producer writes `"rpms_s3_path": null`; the design's Rust struct
writes nothing.

**The gap.** This is a serialize-shape divergence from the Python producer. It
matters more than a generic cross-language difference (which 001:29-32 descopes)
because `BuildArtifactReport` is **not** a Rust-only round-trip format: 001
keeps `report_version` precisely because "the worker and `cbsd-server` already
consume this format" (002:208-212) — i.e. the Rust report is consumed by
components that today read Python-produced reports and may, in a mixed-version
window, read either producer's output. A consumer that distinguishes "key
absent" from "key present and null" (a plausible JSON shape check) sees two
different shapes depending on producer. Even setting that aside, it contradicts
the field-presence parity 002 implies for the other two report optionals (M2).

**Recommended change.** Drop `skip_serializing_if` from `rpms_s3_path`; keep a
bare `Option<String>` (which serde serializes as `null` when `None`, matching
pydantic). If the absent-vs-null distinction is ever deliberately wanted, record
it as an explicit decision with the consumer contract that depends on it; as
written it is an unexamined default.

### Medium M2 — `BuildArtifactReport` optionality is internally inconsistent: three sibling optional fields, three different serde rules

**Design claim.** Within `BuildArtifactReport` (002:185-205) the three
"may-be-absent" fields are given three different treatments:
`container_image: Option<ContainerImageReport>` (bare Option → emits `null`);
`components: Vec<ComponentReport>` with `#[serde(default)]` (bare Vec → emits
`[]`); and `rpms_s3_path` on the nested `ComponentReport` with
`skip_serializing_if` (→ omitted).

**What the Python code actually shows.** All four optional/defaulted fields in
this report tree emit a value under pydantic defaults. Executed on a skipped
report:

```
$ ... B(version='19.2.3', skipped=True).model_dump_json(indent=2)
{ "report_version": 1, "version": "19.2.3", "skipped": true,
  "container_image": null, "release_descriptor": null, "components": [] }
```

`container_image` → `null`, `release_descriptor` → `null`, `components` → `[]`,
and (from M1) `rpms_s3_path` → `null`. Python is uniform: **everything is
present**.

**The gap.** The design maps a uniform Python shape onto three inconsistent Rust
rules. `container_image`/`release_descriptor` (bare Option) and `components`
(bare Vec) happen to match Python; `rpms_s3_path` (the lone
`skip_serializing_if`) does not (M1). Beyond the divergence, the inconsistency
itself is a design smell: an implementer cannot infer the intended convention,
because the document models four equivalent Python fields three different ways
without stating why. This is the non-descopable core of M1 — it survives any
debate about cross-language scope, because it is an internal contradiction
within a single produced type.

**Recommended change.** State and apply one optionality convention for produced
reports: "unset optionals serialize as `null`, defaulted collections serialize
as their empty form" (which matches pydantic and what
`container_image`/`release_descriptor`/`components` already do), and bring
`rpms_s3_path` into line by removing its `skip_serializing_if`. Note the
deliberate exception, if any, rather than leaving it implicit.

### Medium M3 — the shown `VersionType` serde representation is PascalCase; the Python `StrEnum` is lowercase

**Design claim.** 002:216-224 says "The enum and its serde representation live
here" and shows `enum VersionType { Release, Dev, Test, Ci }` with **no**
`#[serde(rename_all = …)]`.

**What the Python code actually shows.** `VersionType(enum.StrEnum)` has
lowercase string values (`versions/utils.py:26-30`): `RELEASE = "release"`,
`DEV = "dev"`, `TEST = "test"`, `CI = "ci"`. Executed: `VersionType.RELEASE`
serializes as `"release"`. The four **variants** the design names are correct
(Release/Dev/Test/Ci ↔ release/dev/test/ci). The defect is the **serde
representation**: a bare serde enum with no `rename_all` serializes its variants
in their Rust identifier casing — `"Release"`, `"Dev"`, `"Test"`, `"Ci"` — which
does not match the lowercase wire values.

**The gap.** The design explicitly scopes "its serde representation" into this
document (002:216-217) and then shows a representation that is wrong-cased. A
caveat worth stating: none of the three produced types in 002
(`VersionDescriptor`, `ReleaseDesc`, `BuildArtifactReport`) embeds
`VersionType`, so this is not a live defect in _those_ serialized shapes today.
But the value is used as the version-store subdirectory name
(`<store>/<type>/<VERSION>.json`, 002:87-89) and is consumed by 006, so the
moment it is serialized — or used as a path segment via its serde/`Display`
value — the casing must be lowercase. Specifying it wrong here, in the SSoT,
guarantees a downstream divergence.

**Recommended change.** Add `#[serde(rename_all = "lowercase")]` to the enum (or
`#[serde(rename = "…")]` per variant) so it serializes as
`release`/`dev`/`test`/`ci`, and align whatever `Display`/`as_str` is used for
the path segment with the same lowercase values. State the wire values
explicitly next to the enum.

### Medium M4 — `NoSuchDescriptor`/`NoSuchVersion` are IO-triggered errors placed in the zero-IO type crate, against 002's own layer split

**Design claim.** 002:226-249 draws a sharp split: "`cbscore-types` holds the
**type-layer** errors — parse/validation failures for the wire types and
version-string errors … no IO. Subsystem error enums that arise from IO or
subprocess work … are defined **with their subsystems**." It then lists, among
the type-layer errors, `NoSuchDescriptor { path }` (analogue of
`NoSuchVersionDescriptorError`) and `NoSuchVersion` (analogue of
`NoSuchVersionError`).

**What the Python code actually shows.** `NoSuchVersionDescriptorError` is
raised **only** from a filesystem `OSError` with `errno == ENOENT` inside
`VersionDescriptor.read` (`versions/desc.py:61-63`) — it is by construction an
IO outcome (file does not exist), not a parse/validation failure of an in-memory
value. `NoSuchVersionError` (`errors.py:36-39`) means "no image descriptor
matched a version"; the design itself notes the resolution that raises it
(`get_image_desc`) "is in 006" (002:248-249) — i.e. it arises in an IO/lookup
subsystem, not the type layer.

**The gap.** Both errors are tied to IO/lookup outcomes, yet are placed in the
zero-IO `cbscore-types` crate, which is in tension with the type-layer-vs-IO
split 002 itself just drew (and with the crate's stated zero-IO charter,
002:10-12). `InvalidDescriptor` (a pydantic `ValidationError` analogue,
`versions/desc.py:65-66`) genuinely is a parse/validation failure and belongs in
the type layer; `NoSuchDescriptor`/`NoSuchVersion` do not fit the criterion the
doc states for type-layer membership.

**Recommended change.** Reconcile the taxonomy with its own rule. Either (a)
move `NoSuchDescriptor` and `NoSuchVersion` to the versions IO subsystem (006),
leaving `cbscore-types` with the genuinely pure members (`MalformedVersion`,
`VersionError`, `InvalidDescriptor`, `UnknownSchemaVersion`); or (b) if a "not
found" _variant_ is wanted in a shared type-layer enum for ergonomics, state
explicitly that the variant is **constructed by** the IO layer (006) and only
_named_ here, so the zero-IO invariant and the split are not contradicted. As
written the doc asserts a clean split and then violates it.

### Low L1 — "unknown marker = hard error" is not expressible by the shown serde attribute

**Design claim.** 002:49-66 shows the marker as
`#[serde(default = "schema_v1")] schema_version: u32` and asserts "Unknown is a
hard error … a higher value is rejected with a typed error
(`UnknownSchemaVersion`)."

**What the mechanism actually does.**
`#[serde(default = "…")] schema_version: u32` deserializes **any** `u32` —
including values greater than the maximum the parser implements. serde has no
built-in "reject if > N" for a plain integer field. The `absent → v1` half of
the claim is correctly expressed by `#[serde(default)]`; the "reject higher"
half is not — it requires an additional, unshown step (a post-deserialize
validation pass, a custom `Deserialize`/`deserialize_with`, or a newtype with a
validating `TryFrom`/`deserialize`).

**The gap.** An implementer transcribing the shown attribute literally gets the
default-to-v1 behavior but silently **not** the hard-error behavior; a v2 file
would parse as `schema_version = 2` and flow downstream unrejected, defeating
the "hard error, not silent mis-parse" guarantee. This is a precision gap in the
machinery spec, not an architectural flaw — the intent is achievable.

**Recommended change.** Show the validating step alongside the attribute: e.g. a
`#[serde(deserialize_with = "…")]` or a post-parse
`if schema_version > MAX { return Err(UnknownSchemaVersion{…}) }` check, and
state where it runs (per-type `validate` after deserialize). Make explicit that
`#[serde(default)]` alone covers only the absent→v1 case.

### Low L2 — `BTreeMap` re-sorts keys; pydantic `dict` preserves insertion order

**Design claim.** 002:141,148,174 use `BTreeMap` for `ReleaseDesc.builds` and
`ReleaseBuildEntry.components`, justified as "deterministic key ordering in the
serialized output."

**What the Python code shows.** The Python types are plain `dict`
(`dict[ArchType, ReleaseBuildEntry]`, `desc.py:128`;
`dict[str, ReleaseComponentVersion]`, `desc.py:116`), which pydantic serializes
in **insertion order**. `BTreeMap` serializes in **sorted-key** order. For
`builds` this is moot today (single arch, `x86_64`). For `components` it is a
latent shape difference: if multiple components are ever inserted in
non-alphabetical order, the Rust output reorders them relative to the Python
producer.

**The gap.** Minor and arguably an improvement (determinism), but it is a
deviation from the Python serialized order that the golden-file-parity claim
(002:264-271) does not carve out. Worth a one-line acknowledgement so the golden
test is written to tolerate (or assert) the sorted order deliberately rather
than discovering the mismatch.

**Recommended change.** Keep `BTreeMap` (determinism is the better property) but
note in the doc that it normalizes key order relative to Python's insertion
order, so the golden-file test compares against sorted-key expectations rather
than asserting byte-equality with arbitrary Python output.

### Low L3 — two error names drop the `Version` token, blurring them against future descriptor types

**Design claim.** 002:242-244 renames `InvalidVersionDescriptorError` /
`NoSuchVersionDescriptorError` to `InvalidDescriptor { path }` /
`NoSuchDescriptor { path }`.

**What the code shows.** The Python errors are specifically about the
**version** descriptor (`versions/desc.py:21-24`, `versions/errors.py:26-51`).
The crate will also hold a release descriptor and (via other designs) container
and image descriptors. The shortened names `InvalidDescriptor` /
`NoSuchDescriptor` no longer say _which_ descriptor.

**The gap.** Cosmetic, but in a crate that defines several descriptor types, an
unqualified `NoSuchDescriptor { path }` is ambiguous at the call site and
invites accidental reuse across descriptor families. Note this interacts with
M4: if `NoSuchDescriptor` moves to 006 it naturally regains the version context.

**Recommended change.** Keep the `Version` qualifier (`InvalidVersionDescriptor`
/ `NoSuchVersionDescriptor`) or otherwise disambiguate, so the type-layer error
names stay 1:1 with their Python analogues and unambiguous among the crate's
descriptor types.

## What the design gets right (verified)

These were checked against the source and should not be re-litigated:

- **`VersionDescriptor` shape.** Field order
  `['version','title','signed_off_by','image','components','distro', 'el_version']`
  matches the design exactly (`versions/desc.py:44-51`, verified via
  `model_fields`); `el_version` is `int` → `u32` is faithful; the `ref` keyword
  rename (`#[serde(rename = "ref")]`, 002:108-109) is correct against
  `VersionComponent.ref` (`desc.py:38-41`). No `model_config` overrides exist on
  any model (verified empty), so the null/default reasoning rests on pydantic
  defaults.
- **`ArchType`/`BuildType`.** Single variants `x86_64`/`rpm` with serde renames
  match the `StrEnum` values (`desc.py:27-33`); keeping them enums for forward
  growth is sound.
- **Single-inheritance flat orders.** `ReleaseBuildEntry` and `ReleaseComponent`
  flattened orders match pydantic output (verified).
- **`report_version` retention.** Keeping the marker name (002:44-46,179-212)
  honors the settled decision in 001 and is correctly framed as intentional, not
  a defect.
- **Error analogues exist.** `MalformedVersionError` (`errors.py:30`),
  `NoSuchVersionError` (`errors.py:36`), `VersionError`
  (`versions/errors.py:20`), `InvalidVersionDescriptorError` /
  `NoSuchVersionDescriptorError` (`versions/errors.py:26,40`) all exist as
  named; the analogue mapping is accurate (the placement is the issue — M4, not
  the existence).
- **`VersionType` variants.** The four variants (Release/Dev/Test/Ci ↔
  release/dev/test/ci) match `versions/utils.py:26-30`; the lookup helper being
  deferred to 006 is consistent with the SSoT framing.
- **Casing invariant.** snake_case for descriptors/report, kebab for
  config/secrets (002:73-83) matches the absence of any `rename_all` on the
  pydantic descriptor models and the settled config/secrets decision.
- **Scope boundaries.** Config/secrets→004, `ContainerDescriptor`→008,
  `ImageDescriptor`→006 deferrals are consistent with 001's subsystem index and
  the settled single-source-of-truth decisions.
- **`absent → v1`.** The `#[serde(default = "schema_v1")]` half is correctly
  expressed (only the "reject higher" half is under-specified — L1).

## Verdict

**GO WITH CHANGES.** The type architecture is sound: the zero-IO charter, the
casing invariant, the schema-versioning policy, the `VersionType` variants, and
the existence and analogue-mapping of the error taxonomy all hold up against the
Python source. Before the `cbscore-types` crate's structs are written from this
document, fold in:

1. **H1 — fix the reversed flatten order.** `ReleaseComponentVersion` is
   BuildInfo-first
   (`arch, build_type, os_version, name, version, sha1, repo_url, artifacts`),
   not header-first; correct the struct (or express it with ordered
   `#[serde(flatten)]`) and pin the correct order in the mixin-flattening test.
2. **M1/M2 — make report optionality consistent and Python-faithful.** Remove
   `skip_serializing_if` from `rpms_s3_path`; state one optionality convention
   (unset → `null`, empty collection → `[]`) and apply it uniformly across the
   four report optionals.
3. **M3 — fix the `VersionType` serde casing.** Add `rename_all = "lowercase"`
   (or per-variant renames) so it emits `release`/`dev`/`test`/`ci`, and align
   the path-segment value.
4. **M4 — reconcile the error split.** Move `NoSuchDescriptor`/`NoSuchVersion`
   to the IO subsystem (006) or explicitly mark them as IO-constructed,
   type-layer-named, so the zero-IO split the doc draws is not self-violated.
5. **L1/L2/L3 — precision.** Show the post-parse "reject higher marker" step
   alongside `#[serde(default)]`; note that `BTreeMap` normalizes key order vs.
   pydantic insertion order so the golden test expects sorted keys; restore the
   `Version` qualifier on the descriptor error names.

None of these is structural; H1 and M1–M3 are concrete wire-shape corrections an
implementer would otherwise transcribe verbatim into the crate, and M4 is an
internal-consistency fix to the document's own stated split. With them folded
in, 002 is ready to drive the `cbscore-types` implementation.
