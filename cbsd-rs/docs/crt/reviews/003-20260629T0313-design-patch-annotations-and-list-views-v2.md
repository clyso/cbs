# CRT — Review: patch annotations & richer `patch list` views (v2)

> **Document:** Design review, seq `003`, v2, 2026-06-29. **Target:**
> `cbsd-rs/docs/crt/design/003-20260628T0807-patch-annotations-and-list-views.md`
> (revised 2026-06-29 addressing review v1) **Prior review:**
> `cbsd-rs/docs/crt/reviews/003-20260628T0818-design-patch-annotations-and-list-views-v1.md`
> **Reviewer:** Staff Engineer / Claude Sonnet 4.6 **Verdict:** GO conditional —
> one specification contradiction must be resolved in §5/§7 before commit 2
> begins; one should-fix (new missing table rows) before commit 3; one
> should-fix (commit 2 size) before implementation starts.

---

## 1. Summary Assessment

The v1 blockers are genuinely resolved and the design is substantially stronger.
§2 now accurately partitions `PatchMeta` into blob-derived vs invocation-derived
fields, §5 carries a complete merge table including the formerly-missing
`Generic-absorbs` and `--unceph-version` rows, §6 pins the JSON element shape
and acknowledges the seq-002 break, and §7 specifies version normalization with
a match matrix that correctly handles pre-release and sub-patch inputs. Three
findings remain or are newly introduced: a self-contradiction in the §5/§7
parser grammar on pre-release inputs (blocker — the tokenizer rejects what the
semantics require), new table gaps for `--unceph-version` applied to
`Generic`/`None` (major — silent wrong result against a named invariant), and an
oversized commit 2 (major — crosses three crates without a clean split). These
are precision fixes on an otherwise sound design; implementation can begin on
commits 1 and 2a after the grammar is corrected.

---

## 2. Resolution Status of v1 Findings

### B1 — §2 "regenerated identically" claim — RESOLVED

The revised §2 accurately distinguishes the six blob-derived fields (stable)
from `provenance` and `source_repo` (last-importer-wins), names the pre-existing
limitation explicitly, and defers the fix. Confirmed against source:
`import_commit` at `import.rs:252-261` constructs `provenance` and `source_repo`
from call-site arguments (`provenance: provenance.clone()`,
`source_repo: source_repo.to_owned()`), while `blob_hash`, `patch_id`,
`subject`, `body`, `author`, and `authored` are all derived from the blob or
commit object. The claim is now accurate.

**Residual (minor):** §2's split into "six blob-derived + two
invocation-derived" is not exhaustive — `PatchMeta` has nine fields
(`meta.rs:51-66`). The ninth, `cherry_picked_from`, is absent from the
enumeration. It is blob-derived (pure function of `body`, computed at
`import.rs:259` via `cherry_picked_from(&body)`) — a reader treating §2 as
exhaustive would wrongly infer it is invocation-derived. One-word fix: add
`cherry_picked_from` to the stable list.

### B2 — §6 `--json` break — RESOLVED

§6 now has a "JSON output schema" subsection that pins:

- Element shape:
  `{ "meta": <PatchMeta>, "annotations": <PatchAnnotations> | null }`
- Flat shape: `[ <element>, … ]`
- Grouped shape: `[ { "group": <string>, "patches": [ <element>, … ] }, … ]`
- Explicit `null` for absent annotations (not absent key)
- The seq-002 break acknowledged and scoped: flat `--json` changes only when the
  element-wrapper lands in commit 2; commit 1's grouped shape is stated to be
  additive

**Residual (minor):** The "commit 1 is additive" framing is partially accurate
but omits that commit 1's grouped `--json` shape will itself churn in commit 2.
Commit 1 emits `[{ "group": <string>, "patches": [PatchMeta] }]`; commit 2
rewrites the inner elements to `{ "meta", "annotations" }`. Any consumer of the
grouped `--json` introduced in commit 1 faces a second breaking change in
commit 2. Since §6 explicitly treats `--json` as pre-stable ("no compatibility
guarantee yet"), this is acceptable — but a note here would make the sequencing
fully transparent.

### M1 — §7 version normalization — SUBSTANTIALLY RESOLVED / ONE CONTRADICTION REMAINS

The revised §7 resolves three of the four v1 gaps:

- **No-patch-component query** (`v18.2`, `18.2`): now classified as a "line"
  query (exactly major.minor). Resolved.
- **Sub-patch versions** (`v18.2.0.1`): `line(x)` is defined as `major.minor`,
  ignoring patch and sub-patch components, so `v18.2.0.1` yields line `18.2`.
  Resolved.
- **v-prefix on query**: leading `v` is stripped before classification.
  Resolved.

**Remaining contradiction (see §3 Blockers):** The pre-release suffix case
(`v18.2.0-rc1`) has a conflict between the tokenizer rule and the match
semantics. The match table in §7 correctly handles it at the semantic level
(under the `[query\spec / point / Exact]` cell, suffix must match exactly), but
the §5 tokenizer rule used by both import and query — "≥ 3 dot-separated numeric
parts" — rejects `18.2.0-rc1` because `0-rc1` is not numeric. The tokenizer and
the semantics disagree on whether `v18.2.0-rc1` is a valid input.

### M2 — §5 merge table — SUBSTANTIALLY RESOLVED / NEW GAP

The revised §5 adds the formerly-missing rows:

- `Generic` + `--ceph-version X` → `Generic` (absorbs, warns, no-op). Resolved.
- `--generic` + `--ceph-version` in one invocation → error (mutually exclusive).
  Resolved.
- `--unceph-version X` with `Versions(S)` → `Versions(S \ {X})`; `∅ ⟹ None`.
  Resolved.

**New gap (see §4 Major Concerns):** Three new `--unceph-version` cases are not
in the table: `Generic --unceph-version X`, `None --unceph-version X`, and the
mutual exclusion rules among `--unassessed`, `--generic`, `--ceph-version`, and
`--unceph-version` within a single `annotate` invocation.

### Minors — ALL RESOLVED

All six v1 minor issues were addressed:

- `schema_version: u32` added to `PatchAnnotations` (§3:69). Confirmed.
- Serde representation pinned with adjacent tagging on `Applicability` and
  `VersionSpec` (§3:76-86). Confirmed — and the choice is technically correct
  (see §2 Strengths).
- `--clear-description` added to `patch annotate` (§5:156). Confirmed.
- O(n) perf note added (§6:209-211, §10). Confirmed.
- Concurrency / last-writer-wins invariant added to §9:306-309. Confirmed.
- Commit 2 now explicitly ships matching unit tests (§8:276-278). Confirmed.

---

## 3. Blockers

### B1. §5/§7 tokenizer self-contradicts on pre-release versions

**The contradiction:** §5 and §7 share the same parser rule: classify an input
by stripping the optional leading `v` and counting "dot-separated **numeric**
parts." For `18.2.0-rc1`, the third segment is `0-rc1`, which is not numeric.
The rule therefore produces fewer than three numeric parts, and under the
catchall "anything else is a hard error," `v18.2.0-rc1` is rejected.

However, §7's match table treats `v18.2.0-rc1` as a valid point query. The
column header is "point `v18.2.1`," and §7:240 lists `v18.2.0-rc1` explicitly as
an input whose line is `18.2`. The match semantics (pre-release suffix must
match exactly for `Exact`, is stripped for `Line`) are fully self-consistent —
but they are only reachable if the tokenizer admits the input, which the current
rule forbids.

**Failure mode:** An implementer following the §5 tokenizer literally writes a
classifier that rejects `v18.2.0-rc1` before reaching the match logic. The
semantic table never fires. Yet the design prose claims the input is valid.
Result: the filtering invariant (RC patches are reachable under `Line("18.2")`)
silently does not hold.

**Direction:** Fix the tokenizer rule. The cleanest formulation:

> Strip an optional leading `v`. Split on `.`. The suffix of the last component
> beginning with `-` (if any) is the pre-release tag; strip it from the
> component for classification purposes. If the remaining dot-separated parts
> are all numeric: count ≥ 3 ⟹ point (Exact); count == 2 ⟹ line (Line); count <
> 2 or any part non-numeric ⟹ hard error. The pre-release tag is preserved in
> the stored or queried value (for Exact match semantics and for `canon()`).

Also state whether a suffix on a line input — `18.2-rc1` (two numeric components
plus a suffix) — is valid. The natural answer is "hard error" (ambiguous:
`18.2-rc1` would nominally be a `Line("18.2")` with an unexplained suffix, and
Ceph never uses that form); stating this explicitly closes the grammar.

---

## 4. Major Concerns

### M1. `--unceph-version` on `Generic` or `None` is unspecified — and `Generic` is a silent-wrong-result trap

The §5 merge table has no rows for:

| Current   | Flag                 | Result |
| --------- | -------------------- | ------ |
| `Generic` | `--unceph-version X` | ???    |
| `None`    | `--unceph-version X` | ???    |

The `None` case is benign: "remove X from an empty set" is a no-op and can be
silently ignored. But the `Generic` case is not safe to silently ignore. If an
operator runs `annotate --unceph-version v18.2.0` on a `Generic` patch expecting
to exclude that version, the current design has no representation for "generic
minus one version" — the no-op leaves `applies_to = Generic`, and the patch
continues to match all versions including `v18.2.0`. This directly violates §9's
"No silent applicability claims" invariant: the operator made a claim the tool
accepted silently and ignored.

**Direction:** The `Generic` + `--unceph-version` case must **error loudly**
with a message such as: "This patch is marked Generic (applies to all versions).
Use `--unassessed` to clear applicability, then `--ceph-version` to set specific
versions before excluding one." The `None` + `--unceph-version` case may either
error (symmetry) or silently no-op (acceptable since `None` makes no claim);
document the choice.

Also add a table row for mutual exclusion among the four `annotate` flags in a
single invocation. §5 states the import-time `--generic`/`--ceph-version`
exclusion. For `annotate`, the interaction of `--unassessed` with
`--ceph-version` or `--generic`, and the interaction of `--generic` with
`--unceph-version`, are unspecified. Recommend: clap `conflicts_with` groups for
the state-reset flags (`--unassessed`) against the state-merge flags
(`--ceph-version`, `--generic`, `--unceph-version`, `--tag`, `--untag`).

### M2. Commit 2 crosses three crates without a clean split — oversized per CLAUDE.md granularity rules

CLAUDE.md (cbsd-rs/) is explicit: "split at clean dependency boundaries; an
independently-testable layer = a separate commit." Commit 2 as defined in §8
bundles:

1. `crt-core`: `PatchAnnotations`, `Applicability`, `VersionSpec`, matching
   logic, table-driven unit tests
2. `crt-store`: `put_annotations`, `get_annotations`, round-trip test
3. `crt` (binary): import bulk flags (`--ceph-version`, `--generic`, `--tag`,
   mutually exclusive enforcement), merge logic, the `--json` element-wrapper
   break, `patch info` annotations rendering

Each of these has a clean dependency boundary and is independently testable.
Commit 2a (`crt-core` types + matching) compiles and tests without any IO or CLI
code. Commit 2b (`crt-store` annotations) is independently testable against
`InMemory`. Commit 2c (`crt` CLI integration) is the only one requiring all
three crates. Bundling all three together creates a large atomic commit that is
harder to review, harder to bisect, and above the CLAUDE.md 400–800 LOC
guidance.

**Direction:** Split commit 2 into three ordered commits along the crate
dependency DAG:

- **2a: `crt-core: add PatchAnnotations, Applicability, VersionSpec, matching`**
  — pure types + matching fn + table-driven unit tests covering all §7 cases. No
  IO, no CLI. Ships the matching fn with its caller (the tests), resolving the
  v1 seam concern while keeping the commit self-contained.
- **2b: `crt-store: add put_annotations / get_annotations`** — store trait
  extension + implementation + round-trip test. Depends on 2a; no CLI.
- **2c: `crt: import bulk flags, merge, JSON break, patch info annotations`** —
  CLI integration: `--ceph-version`/`--generic`/`--tag` on import, merge per §5,
  `--json` element-wrapper, `patch info` annotations rendering. Depends on 2b.

Also consider whether §8's parenthetical "(2) and (3) may merge if (3) is small"
is still appropriate — commit 3 (`annotate` subcommand) is now well-scoped and
self-contained; merging it into the already-heavy 2c would recreate the very
problem the split addresses.

---

## 5. Minor Issues

- **§2 `cherry_picked_from` unlisted.** The §2 split enumerates six blob-derived
  fields and two invocation-derived fields — nine total — but `PatchMeta` has
  nine fields (`meta.rs:51-66`), and `cherry_picked_from` is not listed. It is
  blob-derived (pure function of `body`). A reader treating §2's split as
  exhaustive would wrongly infer it is invocation-derived. Add it to the stable
  list.

- **Commit 1 grouped `--json` shape churns in commit 2.** §6 says commit 1 is
  "additive" — the flat `--json` is unchanged. That is correct for the flat
  path. But commit 1's grouped path emits
  `[{ "group", "patches": [PatchMeta] }]`, which commit 2 changes to
  `[{ "group", "patches": [element] }]`. A consumer of commit 1's grouped
  `--json` faces a second break in commit 2. Since `--json` is pre-stable this
  is acceptable; a one-line note in §6 would close the gap.

- **Group membership for multi-value patches.** §6 specifies
  `--group-by ceph-version` and `--group-by tag` but does not define group
  membership for patches that match multiple values. A patch in
  `Versions({Line("18.2"), Line("19.0")})` appears in which `ceph-version`
  groups — both? one? The same question applies to a patch with multiple tags
  under `--group-by tag`. Either policy (all-matching, first-matching) is
  defensible; the choice needs to be stated.

- **`Exact("v18.2.0")` round-trip under `--unceph-version v18.2.0`.** The §5
  parser describes `--ceph-version 18.2.0` (no leading `v`) as being stored
  v-prefixed. If the operator later runs `--unceph-version 18.2.0`, the same
  parser produces `Exact("v18.2.0")` and the set subtraction `{spec(X)}` must
  find and remove the stored `Exact("v18.2.0")`. This requires the parsing of
  the `--unceph-version` argument to produce the same canonical form as the
  original `--ceph-version` argument — which the grammar does, as written. No
  bug, but the round-trip property is implicit; a single test case in the §7
  unit tests for the remove path would make it explicit.

---

## 6. Strengths (additions since v1)

In addition to the v1 strengths (separation premise, None ⟹ unassessed, zero
migration, crt-core purity, attributes escape hatch, commit 1 independence):

- **Adjacent tagging on `Applicability`/`VersionSpec` is technically correct.**
  Internal serde tagging (`#[serde(tag="type")]`) requires struct/map variant
  payloads. `Applicability::Generic` (unit variant — no payload), and
  `VersionSpec::Line`/`Exact(String)` (newtype → scalar payload) are not
  structs. Applying internal tagging to them would be a serde compile-time error
  or incorrect output. Adjacent tagging (`tag`+`content`) is valid for every
  variant kind — unit, newtype, tuple, struct — and is the right choice.
  Confirmed against `meta.rs:34` (`Provenance`'s variants are both structs,
  explaining why it can use internal tagging). The deviation from v1's
  suggestion was technically correct and the design's rationale for it is
  accurate.

- **§5 merge table now complete for the primary cases.** The `Generic` absorbs
  `--ceph-version`, the mutual-exclusion on `--generic`+`--ceph-version`, and
  the `Versions(∅) ⟹ None` normalization are all present. The table is clear and
  implementable for the common paths.

- **§9 concurrency invariant.** The single-writer acknowledgment, with an
  explicit analogy to `import_commit`'s own non-transactional note and to
  seq-001 §5's future-service seam, is the right level of documentation for a
  CLI-era tool.

- **§7 match table is self-consistent.** The four cells (point×Line,
  point×Exact, line×Line, line×Exact) are all correctly specified including the
  pre-release mismatch case (`Exact("v18.2.0")` does NOT match point query
  `v18.2.0-rc1`). The definitions of `line(x)` and `canon(x)` are clear. Only
  the tokenizer (§3, above) needs repair.

---

## 7. Open Questions

1. **`18.2-rc1` as an input.** Is a line-shaped input with a pre-release suffix
   a hard error (recommended) or a synonym for `Line("18.2")`? The grammar does
   not cover this case.

2. **`--unassessed` and state-merge flags in one `annotate` call.** Can an
   operator run `crt patch annotate <hash> --unassessed --tag rgw`? The
   `--unassessed` resets `applies_to` to `None`; `--tag` unions into `tags`. Are
   they composable within one command (applies_to reset, tags merged), or does
   `--unassessed` conflict with everything? This is an ergonomics question, not
   a correctness question, but the answer should be in the table.

---

## 8. Confidence Score

| Item                                                                               | Points | Reason                                                                              |
| ---------------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------- |
| Starting score                                                                     | 100    |                                                                                     |
| B1 (v1) §2 provenance/source_repo                                                  | 0      | Resolved — no deduction                                                             |
| B2 (v1) --json break unacknowledged                                                | 0      | Resolved — no deduction                                                             |
| M1 (v1) normalization underspecified (3 of 4 sub-cases)                            | 0      | Resolved — no deduction                                                             |
| M2 (v1) merge table gaps (Generic, conflicting flags, unceph-version)              | 0      | Resolved — no deduction                                                             |
| Minors (v1 schema_version, serde, clear-description, perf, concurrency, seam)      | 0      | All resolved — no deductions                                                        |
| **B1 (new):** §5/§7 tokenizer rejects pre-release, contradicts match semantics     | -15    | Spec contradiction — tokenizer and semantics disagree on a named input class        |
| **M1 (new):** `--unceph-version` on Generic is silent wrong result vs §9 invariant | -10    | Behavioral gap — violates named invariant ("No silent applicability claims")        |
| **M2 (new):** Commit 2 oversized, crosses three crate boundaries without split     | -8     | Commit boundary — CLAUDE.md granularity rules require splitting at dependency seams |
| Minor: `cherry_picked_from` missing from §2 field enumeration                      | -3     | Documentation accuracy                                                              |
| Minor: Commit 1 grouped --json churns silently in commit 2                         | -3     | Pre-stable but unacknowledged second break on same path                             |
| Minor: Group membership for multi-value patches unspecified                        | -3     | Spec gap — implementer has no guidance                                              |
| Minor: --unceph-version on None unspecified (benign; policy choice)                | -2     | Documentation gap                                                                   |
| **Total**                                                                          | **56** |                                                                                     |

**Score: 56 / 100 — GO conditional.**

The dominant deductions are a spec contradiction (B1) and a behavioral gap with
an invariant violation (M1). Both are precision repairs to the §5/§7 grammar and
table, not architectural rethinks. Implementation can begin on commit 1
immediately (fully independent, no annotations, no issues). Commit 2a
(`crt-core` types and matching) can begin after B1 is repaired. Commits 2b, 2c,
and 3 follow in sequence.

---

## 9. Required Actions Before Implementation (by commit)

**Before commit 1:** None — proceed.

**Before commit 2a:**

1. **[Blocker] Fix §5/§7 tokenizer rule** to handle `v18.2.0-rc1` and family:
   separate the pre-release suffix from the numeric part count classification.
   State whether `18.2-rc1` (two numeric parts + suffix) is a hard error.

**Before commit 3:**

2. **[Major] Add missing `--unceph-version` table rows for `Generic`/`None`** to
   §5. The `Generic` case must error loudly; the `None` case needs a stated
   policy. Add the `annotate`-time mutual exclusion rules among state-reset and
   state-merge flags.

**Before commit 2a (also applies to 2b, 2c):**

3. **[Major] Split commit 2 into three commits:** 2a (`crt-core` only), 2b
   (`crt-store` only), 2c (`crt` CLI integration). Update §8 accordingly. Remove
   or requalify the "(2) and (3) may merge" parenthetical.

**Before committing any markdown:**

4. **[Minor] Add `cherry_picked_from` to §2's stable-field list.**
5. **[Minor] Note in §6 that the grouped `--json` shape also churns in
   commit 2.**
6. **[Minor] State group-membership policy for multi-value patches in §6.**
