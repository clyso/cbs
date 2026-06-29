# CRT — Review: patch annotations & richer `patch list` views

> **Document:** Design review, seq `003`, v1, 2026-06-28. **Target:**
> `cbsd-rs/docs/crt/design/003-20260628T0807-patch-annotations-and-list-views.md`
> **Reviewer:** Staff Engineer / Claude Sonnet 4.6 **Verdict:** NO-GO — revise
> before implementing.

---

## 1. Summary Assessment

The core architecture is sound: a separate, mutable annotations record keyed by
blob hash, merged on import, with `None` meaning "unassessed" rather than
"generic" — all three are good decisions. The design is not a rethink; it needs
targeted revision on four load-bearing points before implementation begins.

Two are blockers: the §2 separation premise rests on a claim that is only
partially true (meta is not always regenerated identically), and the `--json`
shape change in §6 breaks seq-002's shipped contract without acknowledgment. Two
others are major: the matching normalization algorithm in §7 has undefined
behavior on realistic inputs (pre-release tags, no-patch-component queries), and
the merge table in §5 leaves the `--ceph-version`-when-Generic and
conflicting-flags cases unspecified. The commit plan is sound in structure but
has a seam imprecision worth correcting.

---

## 2. Strengths

- **Separation premise (correct conclusion, partially wrong rationale).** The
  key insight — mutable operator metadata must live at a different path from
  git-derived meta so import never clobbers it — is correct, well-motivated, and
  architecturally clean. Confirmed in source: `put_meta` at `import.rs:269` is
  an unconditional plain `.put()`, and annotations live at a distinct key
  prefix. The conclusion holds even though the supporting rationale has a gap
  (see §3).

- **`None` ⟹ "unassessed", not "generic."** Excluding `None` from version
  filters and from `--generic` is the right call. The invariant ("No silent
  applicability claims") is spelled out explicitly and belongs in §9.

- **Absent record ⟹ zero migration.** `get_annotations → Result<Option>` means
  stores that predate this design need no migration: a missing annotations key
  returns `None`, which the caller interprets as unassessed defaults. This is
  consistent with existing `get_patch_id` usage in `crt-store/src/lib.rs` (lines
  75, 301). Credit it explicitly as a design strength — the absence path is not
  a gap, it is the mechanism.

- **`crt-core` stays pure.** Types and matching logic in `crt-core`, IO in
  `crt-store`, CLI in `crt` — the layering matches the project convention.

- **`attributes` escape hatch with a graduation path.** The open
  `BTreeMap<String, String>` bag for unstructured metadata, with a stated path
  to typed fields, is the right balance between flexibility and eventual schema
  health.

- **Commit 1 is cleanly independent.** `--group-by pr|source-repo` operates
  entirely on existing `PatchMeta` fields, touches only `crt`, and has no
  dependency on anything below it. This is a textbook single-capability commit.

---

## 3. Blockers

### B1. §2's "regenerated identically" claim is partially false — meta has a latent lossy-overwrite

**What the design says:** §2 asserts that `put_meta`'s unconditional overwrite
"is safe" because `PatchMeta` is "regenerated identically on every import."

**What the code actually does:** `import_commit` in `import.rs` derives
`blob_hash`, `patch_id`, `subject`, `body`, `author`, and `authored_at` from the
commit object — those _are_ deterministic from the blob. But `provenance` and
`source_repo` come from CLI invocation arguments, not from the blob content:

- A PR import (`import_pr`) records
  `Provenance::UpstreamPr { prs, commits, state }` and
  `source_repo = "ceph/ceph"` (or whatever the remote is).
- A range import (`import_commit` on a range with `--range`) records
  `Provenance::Other { description }` and `source_repo = "/local/path/to/repo"`
  (per `main.rs:333`).

Same blob, different provenance fields. The second import unconditionally
overwrites the first. A curator who imports a cherry-picked commit from a local
range, having already imported it via PR, silently loses the PR provenance — the
patch's upstream lineage is gone.

**Why it matters:** The safety argument for `put_meta`'s overwrite is the
centerpiece of §2's separation rationale ("this is why we don't need merge for
meta"). That argument is wrong for `provenance`. The latent lossy-overwrite
pre-exists this design, but the design amplifies the risk by stating the
overwrite is unconditionally safe. Downstream designs (e.g., the `release add`
guard in §10) may rely on `provenance` being accurate.

**Direction:** Revise §2 to narrow the claim accurately: "fields derived solely
from the blob bytes (hash, patch_id, subject, body, author, authored_at) are
stable across imports; fields derived from invocation context (provenance,
source_repo) are overwritten on re-import — last importer wins." Optionally note
this as a known limitation; the fix (merge semantics for meta) is out of scope
here. Do not claim unconditional idempotence. The annotations separation still
stands on its own merits.

---

### B2. §6's `--json` output breaks seq-002's shipped contract without acknowledgment

**What seq-002 ships:** `patch list --json` emits `Vec<PatchMeta>` (confirmed in
`patch.rs`'s `list_json_round_trips` test). Consumers depend on that shape.

**What §6 proposes — without naming the break:**

1. Each element becomes a `{PatchMeta, PatchAnnotations?}` wrapper instead of a
   bare `PatchMeta`.
2. With `--group-by`, the top-level shape changes from an array of patches to an
   array of `{group, patches}` objects.

Both changes are breaking. The design never flags them as such, doesn't pin
field names for the wrapper type (`patch`? `meta`? `annotation`? `flattened?`),
and doesn't specify how a consumer handles the polymorphic top-level shape (must
branch on whether `--group-by` was passed).

**Why it matters:** The seq-002 `--json` output is documented and testable.
Silent breaking changes to CLI output violate the seq-002 invariant ("Result on
stdout, diagnostics on stderr") and create a versioning debt that grows with
every downstream consumer.

**Direction:** Add a §6 subsection on JSON schema stability:

- Define the wrapper type explicitly (field names, whether `PatchAnnotations` is
  nullable or absent-key).
- Define the `--group-by` grouped shape explicitly
  (`{group: string, patches: [...]}`).
- State that commit 1 (grouping) changes the `--json` schema for `patch list`
  and that this is a breaking change — consumers must update.
- Consider versioning or a `--json-version` flag if backward compatibility is
  required.

---

## 4. Major Concerns

### M1. §7 version normalization algorithm is underspecified for realistic Ceph inputs

The design states: "`Line(s)` matches iff `q`, normalized by stripping a leading
`v` and its patch component, equals `s`."

That algorithm is undefined or ambiguous for:

1. **Query with no patch component:** `--ceph-version v18.2` or
   `--ceph-version 18.2`. "Strip the patch component" when there is no patch
   component — does the query match `Line("18.2")`? Reject as malformed? Treat
   as `18.2.0`?
2. **Pre-release suffix:** Ceph ships `v18.2.0-rc1`. After stripping the `v` and
   patch component, the remainder is `18.2-rc1`, not `18.2`. Does `Line("18.2")`
   match `v18.2.0-rc1`?
3. **Sub-patch versions:** `v18.2.0.1` (used by some downstream packagers). The
   "patch component" is ambiguous for a 4-part version.
4. **Exact vs no-v-prefix query:** The design says `Exact(s)` compares "as
   written, v-prefixed." If the operator passes `--ceph-version 18.2.0` (no
   `v`), does it match `Exact("v18.2.0")`?

These are not edge cases — Ceph release tooling encounters pre-release versions
regularly.

**Direction:** Specify a normalization grammar. Recommend: define a
`VersionQuery` parser (separate from `VersionSpec`) in `crt-core` that:

- Accepts an optional leading `v` (strips it before comparison)
- Parses `MAJOR.MINOR.PATCH[-PRERELEASE]` with defined handling of the
  pre-release suffix (strip it for `Line` matching, preserve it for `Exact`
  matching)
- Rejects (with a clear error) anything that cannot be parsed
- Documents what "patch component" means for non-3-part versions

The matching unit tests in commit 2 (or 4) should cover all four cases above as
table-driven tests.

---

### M2. §5 merge table leaves two cases unspecified — conflicting behaviors at import time

**Case 1: `--ceph-version` applied when existing state is `Generic`.** The
design says "`--generic` sets Generic (Generic absorbs any Versions)" but does
not say what happens in reverse: if the existing record is `Generic` and the
operator runs `import --ceph-version v18.2.1`. Is `Generic` preserved (absorbs
the specific version)? Is it replaced? The design's union-semantics suggest
preservation, but that is never stated.

**Case 2: `--ceph-version` and `--generic` in the same invocation.**
`crt patch import ... --ceph-version v18.2.1 --generic` — which wins? Clap will
accept both flags. The merge table defines each flag's individual behavior but
not their interaction. The operator intent is ambiguous; the CLI should either
reject the combination or document a deterministic resolution order.

**Case 3: Removing a single `VersionSpec`.** There is no way to remove one
version from `Versions(...)` without resetting the entire `applies_to` to `None`
via `--unassessed`. The `patch annotate` command has `--untag <t>`
(additive/subtractive symmetry for tags) but no equivalent for
`--unceph-version <v>`. This may be intentional, but it should be stated.

**Direction:** Add a merge-semantics table to §5 covering:

| Current state         | Flag(s)                        | Result                                |
| --------------------- | ------------------------------ | ------------------------------------- |
| `None`                | `--generic`                    | `Some(Generic)`                       |
| `None`                | `--ceph-version v18.2.1`       | `Some(Versions({Exact("v18.2.1")}))`  |
| `Some(Versions(...))` | `--generic`                    | `Some(Generic)` (absorbs)             |
| `Some(Generic)`       | `--ceph-version v18.2.1`       | `Some(Generic)` (preserved — absorbs) |
| any                   | `--generic` + `--ceph-version` | error: mutually exclusive             |

And state explicitly whether removing a single `VersionSpec` is out of scope.

---

## 5. Minor Issues

- **No `schema_version` on `PatchAnnotations`.** `PatchMeta` has none (it is
  regenerable). `Manifest` has one (it is not regenerable). `PatchAnnotations`
  is operator-authored and non-regenerable — it belongs in the `Manifest` camp.
  The design already anticipates field graduation ("graduates to a typed field
  then"), which implies schema evolution. Without a version field, a future
  deserialization of a stored record after a structural change will silently
  misparse. Cheap to add now; expensive after operators have authored records in
  production. Recommend adding `schema_version: u32 = 1` to the struct.

- **Serde representation of `Applicability`/`VersionSpec` not pinned.** Both
  enums are persisted to JSON. The `Provenance` enum in `crt-core/src/meta.rs`
  uses explicit `#[serde(tag = "type")]` annotations to stabilize wire
  representation. `Applicability` and `VersionSpec` need the same treatment, or
  the default representation (Rust enum variant names) becomes an implicit
  contract that is easy to break with a rename.

- **`description` has no clear-to-None path.** `--description <text>` sets the
  field; there is no `--clear-description` or `--description ""` semantics
  defined. The tags have `--untag`; attributes have `--unset`. The asymmetry may
  be intentional but should be stated.

- **`--unassessed` / `--ceph-version` filter performance.** For `--unassessed`
  specifically, the design routes through the meta prefix (`list_patches`
  returns all blob hashes, then `get_annotations` per hash). Un-annotated blobs
  (the common case for `--unassessed`) each result in a 404 from the annotations
  prefix. For a store with thousands of patches, that is O(n) 404s. An
  alternative — listing the annotations prefix once, diffing against the meta
  prefix to find absent keys — is cheaper and worth noting even if deferred to
  §10.

- **`patch info` annotations rendering format not defined.** §6 says
  "`patch info` shows the merged view ... applicability, tags, description, and
  attributes" but does not specify the terminal format. This is intentionally
  deferred ("exact format finalized in the plan") for list, but info has no
  equivalent note. Minor — just note it as plan-time, or add a sketch.

- **Commit 2 matching logic placement — seam concern, not dead code.** The
  applicability matching function (`fn matches(query: &str) -> bool` or
  equivalent) in `crt-core` has no production caller until commit 4's
  `--ceph-version` filter. As a `pub` function in a library crate it will not
  trigger `dead_code` lint and needs no `#[allow]`. The smell-test question is
  whether it has unit tests in commit 2 — if it ships with tests, it is a tested
  library API (acceptable). If it ships with no test and no caller, it is a
  feature-seam violation. Recommend: either move matching to commit 4 for a
  self-contained seam (types, storage, import flags, and display in commit 2;
  matching and filtering in commit 4), or ensure commit 2 includes matching unit
  tests explicitly.

---

## 6. Suggestions

- **Make `--group-by` clap enum declare `ceph-version` and `tag` variants in
  commit 1**, gated behind a runtime error ("not yet available — requires
  annotations support"). This avoids mid-stream enum extension and makes commit
  1's `--group-by` contract complete. Optional — deferring the enum extension is
  equally valid if the plan commits to adding it in commit 4.

- **`base_ref` auto-derive is partially feasible now.** The `import_pr` path
  already fetches `base_ref` (confirmed at `import.rs:87-91`). Deferring
  auto-derivation is reasonable (the design correctly prioritizes
  operator-assertion), but it is worth noting in §5's future note that the PR
  path already has the data — only the range path would need heuristics.

---

## 7. Open Questions

1. **Empty `Versions(∅)` — is it valid?** If both `--unassessed` and
   `--ceph-version` flags are used in a sequence that produces an empty set, is
   `Versions({})` a meaningful state? Should it be normalized to `None`?

2. **`--ceph-version` input format:** The design accepts `18.2` (line) and
   `v18.2.0` (exact). How does the CLI distinguish? By counting dots? By
   presence/absence of `v`? By number of numeric components? Specify the parsing
   rule.

3. **`--json` compatibility guarantee:** Is seq-002's `--json` output considered
   stable (backward-compatible changes only), or is it considered unstable until
   explicitly versioned? The answer determines whether the schema change in §6
   is a breaking change or a pre-stable iteration.

4. **Concurrent `crt patch annotate` invocations:** The read-modify-write in
   `put_annotations` is last-writer-wins with no optimistic locking or CAS. For
   a CLI tool used by a single operator against a local or S3 store, this is
   acceptable. Is the concurrency model expected to remain single-writer, or
   will multi-operator workflows be supported? (seq-001 §5 presents the store as
   a future service seam.) A note in §9 invariants analogous to
   `import_commit`'s own non-transactional acknowledgment would close this.

---

## 8. Confidence Score

| Item                                                                              | Points | Reason                                                                        |
| --------------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------- |
| Starting score                                                                    | 100    |                                                                               |
| B1: §2 "regenerated identically" claim false for `provenance`/`source_repo`       | -15    | Spec deviation — central safety argument is inaccurate; not a schema gap      |
| B2: `--json` breaking change to seq-002 contract unacknowledged                   | -15    | Spec deviation — shipped contract broken silently, field names undefined      |
| M1: `Line` normalization undefined for pre-release, no-patch-component, sub-patch | -10    | Spec deviation — three distinct unspecified inputs on realistic Ceph versions |
| M2: `--ceph-version`-when-Generic case unspecified                                | -5     | Spec deviation                                                                |
| M2: `--generic` + `--ceph-version` same invocation unspecified                    | -5     | Spec deviation                                                                |
| M2: No mechanism to remove one `VersionSpec` (asymmetry with `--untag`)           | -5     | Missing documentation                                                         |
| Minor: No `schema_version` on non-regenerable `PatchAnnotations`                  | -5     | Data structure                                                                |
| Minor: `Applicability`/`VersionSpec` serde representation not pinned              | -5     | Spec deviation — persisted enum with implicit wire format                     |
| Minor: Concurrent RMW not acknowledged in §9 invariants                           | -5     | Missing documentation                                                         |
| Minor: Commit 2 matching logic — seam imprecision if no unit tests ship           | -5     | Commit boundary (conditional; avoidable if tests are included)                |
| **Total**                                                                         | **25** |                                                                               |

**Score: 25 / 100 — Major rework needed. Block implementation.**

The dominant deductions are specification correctness, not implementation
uncertainty. The architecture is right; the written spec has enough ambiguity
and one incorrect load-bearing claim that implementation against it would
produce either bugs (B1, M1) or a silent contract break (B2). Revise §2, §5, §6,
and §7 per the blockers and major concerns above, then re-review.

---

## 9. Required Actions Before Implementation

1. **[Blocker] Revise §2** to narrow the "safe overwrite" claim: state which
   fields are deterministic from the blob (stable) and which come from
   invocation context (`provenance`, `source_repo` — last importer wins). Do not
   claim unconditional idempotence.

2. **[Blocker] Revise §6** to define the `--json` output schema explicitly:
   wrapper field names, nullable vs absent `PatchAnnotations`, and the grouped
   shape. Acknowledge that this is a breaking change to seq-002's `--json`
   contract.

3. **[Major] Revise §7** to specify the normalization grammar precisely:
   pre-release suffix handling, no-patch-component query behavior, sub-patch
   version handling, and v-prefix requirements for `Exact` matching.

4. **[Major] Revise §5** to add the merge-semantics table covering
   `--ceph-version`-when-Generic, mutually exclusive flag combinations, and
   whether removing a single `VersionSpec` is in scope.

5. **[Minor] Add `schema_version: u32`** to the `PatchAnnotations` struct
   definition in §3 and note the serde representation strategy for
   `Applicability`/`VersionSpec` enums.

6. **[Minor] Commit 2 unit tests:** ensure the commit plan explicitly requires
   matching logic unit tests (table-driven, covering the M1 cases) to land in
   the same commit as the matching function, or move the matching function to
   commit 4.
