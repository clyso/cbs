# CRT — Review: patch annotations & richer `patch list` views (v3)

> **Document:** Design review, seq `003`, v3, 2026-06-29. **Target:**
> `cbsd-rs/docs/crt/design/003-20260628T0807-patch-annotations-and-list-views.md`
> (revised 2026-06-29 addressing design reviews v1 and v2) **Prior reviews:** v1
> `003-20260628T0818-…-v1.md` (NO-GO, 25/100); v2 `003-20260629T0313-…-v2.md`
> (GO conditional, 56/100) **Reviewer:** Staff Engineer / Claude Sonnet 4.6
> **Verdict:** GO — implementation can begin on commit 1 now and proceed down
> the DAG. Four minor notes; none block.

---

## 1. Summary Assessment

All v2 findings are genuinely resolved. The §5/§7 tokenizer now handles every
input class without contradiction; the `Generic --unceph-version` gap was filled
with a loud error that preserves the §9 invariant; commit 2 was split correctly
along the crate dependency DAG; and all four minors from v2 landed in the text.
The two v2 open questions are answered.

One new substantive find: the §5 future-note overestimates the feasibility of
auto-deriving `applies_to` from the PR `base_ref` by conflating a branch name
with a version string. The note is out of scope, so it does not block. Three
additional cosmetic notes (text/json asymmetry explanation, None-no-op
rationale, two nits) are offered as improvements.

The architecture is correct, the grammar is total, the merge semantics are
complete, the commit plan is well-structured, and the invariants are upheld.

---

## 2. Resolution Status of v2 Findings

### v2 B1 — Tokenizer self-contradiction on pre-release — RESOLVED

The grammar in §5 (version parsing) now separates the pre-release suffix
(`-rc1`) from the numeric core before counting dot-separated parts. Verified
against all six input classes:

| Input                   | Strip `v` / tag   | Numeric core parts | Classification                      |
| ----------------------- | ----------------- | ------------------ | ----------------------------------- |
| `v18.2.0-rc1`           | `18.2.0` + `-rc1` | 3                  | `Exact("v18.2.0-rc1")`              |
| `v18.2.0`               | `18.2.0`          | 3                  | `Exact("v18.2.0")`                  |
| `18.2.0` (no `v`)       | `18.2.0`          | 3                  | `Exact("v18.2.0")`                  |
| `v18.2.0.1`             | `18.2.0.1`        | 4 (≥3)             | `Exact("v18.2.0.1")`, line = `18.2` |
| `18.2`                  | `18.2`            | 2                  | `Line("18.2")`                      |
| `18.2-rc1`              | `18.2` + `-rc1`   | 2 + tag            | **hard error** (explicitly stated)  |
| `squid`, `18`, `v18.2.` | non-numeric or <2 | —                  | **hard error**                      |

The grammar is now total (every input either classifies or is a defined error)
and consistent with the §7 match table. `v18.2.0-rc1` is admitted as a point
query and correctly handled: `Exact("v18.2.0")` does NOT match it (suffix
differs), but `Line("18.2")` does (suffix stripped for line derivation). The
`--unceph-version` round-trip works: `--ceph-version 18.2.0` stores
`Exact("v18.2.0")`; `--unceph-version v18.2.0` parses to the same canonical
form.

### v2 M1 — `--unceph-version` on Generic/None; flag exclusion — RESOLVED

The §5 merge table now has all three formerly missing rows:

- `None --unceph-version X` → `None` (no-op: nothing to remove, no claim made)
- `Generic --unceph-version X` → **error** with remediation message (§5:181-184)
- `any --unassessed` → `None` (already present)

§9 (§9:340-342) updated: "Generic --unceph-version errors rather than silently
no-op'ing." The `Generic` case cannot silently succeed — there is no "Generic
minus one version" representation, so accepting the flag without an error would
leave the patch matching all versions while the operator believes one was
excluded. This directly violates the "No silent applicability claims" invariant;
the loud error upholds it.

Flag exclusion is now fully specified (§5:191-198): the four `applies_to`
operations (`--generic`, `--unassessed`, `--ceph-version`, `--unceph-version`)
are mutually exclusive per invocation via clap `conflicts_with`. They remain
orthogonal to tag/description/attribute flags:
`annotate <h> --unassessed --tag rgw` is valid (applies_to reset, tag merged).
Both v2 open questions resolved by the same paragraph.

### v2 M2 — Commit 2 oversized — RESOLVED

§8 now specifies six commits along the crate DAG:

1. `crt` — grouping (PR/source-repo), no store or core change
2. `crt-core` only — types, matching, table-driven unit tests
3. `crt-store` only — `put_annotations`/`get_annotations`, round-trip test
4. `crt` — import flags, merge, JSON wrapper break, `patch info` rendering
5. `crt` — `patch annotate` subcommand (edit annotations)
6. `crt` — filters and `--group-by ceph-version|tag`

Each commit compiles independently, is testable in isolation, and ships its own
tests. Commit 2a is a tested-library increment (matching function ships with its
table-driven tests covering every §7 case plus the `--unceph-version`
round-trip), not a dead-code layer. The §8 rationale explicitly states why 3 is
kept separate ("folding it into the heavier 2c would re-create the oversized
commit this split avoids"). The parenthetical that opened the door to merging
commits 2 and 3 is gone.

### v2 Minors — ALL RESOLVED

- **`cherry_picked_from` in §2:** Added to the blob-derived field list at §2:36.
  Confirmed accurate — it is computed at `import.rs:259` as
  `cherry_picked_from(&body)`, a pure function defined at `meta.rs:70-79`.
- **Commit 1's grouped `--json` churns in commit 2c:** §6:256-258 now states
  this explicitly ("Commit 1's grouped shape also gains the wrapper then — a
  second change on the grouped path, acceptable only because `--json` is
  pre-stable").
- **Group membership for multi-value patches:** §6:218-221 specifies
  all-matching membership (`Generic` patches form a `(generic)` group;
  unassessed form `(unassessed)`; a multi-version or multi-tag patch appears in
  each matching group).
- **`None --unceph-version` policy stated:** §5:172 ("no-op — nothing to
  remove"). Resolved.

---

## 3. Blockers

None.

---

## 4. Minor Issues

### N1. §5 future note overstates feasibility of base_ref auto-derive

§5 (lines 200-201) says the PR `base_ref` is "already fetched
(`import.rs:87-91`), so `applies_to` could be suggested from it." The PR
`base_ref` is the PR's **base branch name** — `"squid"`, `"main"`, `"reef"` —
not a version string. The §5 parser (strip `v`, split on `.`, require numeric
parts) rejects all of these as hard errors. Auto-deriving a `VersionSpec` from a
branch name requires a separate branch→line map (e.g. `"squid"` → `"19.2"`,
`"reef"` → `"18.2"`), not the parser. The note is out of scope and the
inaccuracy does not block implementation; but it overstates "feasibility" and
could mislead whoever picks up the future work.

Note: the **release** `base_ref` (§7:264, `crt-store/src/lib.rs:487`) is
version-shaped (`"v18.2.0"`) and is correctly handled by the parser — only the
PR `base_ref` is a branch name.

**Direction:** Amend the §5 future note to read "...the PR `base_ref` is a
branch name (`squid`, `reef`), not a version; auto-deriving `VersionSpec` from
it requires a branch→version-line map. The range path would additionally need a
heuristic on the range base."

### N2. Text/JSON asymmetry across commits 2c and 4 is not called out

Commit 2c surfaces annotations in `patch list --json` (the element wrapper)
immediately after annotations exist. The annotations-aware **text** line
("`<blob_hash>  <subject>  [18.2 | rgw]`") is deferred to commit 4. This means
the JSON and text outputs are briefly inconsistent: between commits 2c and 4,
`--json` includes annotation data that the default text output does not show.
The asymmetry is defensible (JSON reflects the data as soon as it is written;
text waits for the full filtering/grouping context), but the design does not
explain it. Without a note, an implementer or reviewer in commit 2c might read
the missing text line as an oversight.

**Direction:** One sentence in §8 commit 2c noting that the annotations-aware
text line lands in commit 4 and explaining why (the display format depends on
filter context defined there).

### N3. None-no-op vs Generic-error asymmetry has no stated rationale

§5 is correct: `Generic --unceph-version` errors loudly; `None --unceph-version`
silently no-ops. But the asymmetry is not explained in the text. An operator or
implementer reading the table will notice that the two "empty of specific
versions" states are handled differently and may wonder if the `None` row is a
mistake.

**Direction:** One sentence: "`None` asserts no applicability claim, so a no-op
leaves the invariant intact; `Generic` asserts 'all versions', so accepting
`--unceph-version` without an error would silently contradict it."

### N4. Two nits

- **§5:165 table caption** says "identical for import and `annotate`" but import
  does not have `--unceph-version` or `--unassessed` rows (those are
  `annotate`-only per §5:1). The table covers all rows; the caption should say
  "applies_to transitions (import: first three rows active; annotate: all
  rows)."
- **`--group-by ceph-version` group key rendering** for stored `Exact` specs
  (`"v18.2.0"`) is not pinned — when grouping by version, does a patch with
  `Exact("v18.2.0")` appear under group key `"v18.2.0"` or `"18.2"`? Most likely
  the stored value; stating it is a plan-time detail is fine, but a one-line
  note closes the question.

---

## 5. Confidence Score

| Item                                                          | Points | Reason                                                                 |
| ------------------------------------------------------------- | ------ | ---------------------------------------------------------------------- |
| Starting score                                                | 100    |                                                                        |
| v1 B1 (provenance claim)                                      | 0      | Resolved in v2                                                         |
| v1 B2 (JSON schema unspecified)                               | 0      | Resolved in v2                                                         |
| v1 M1 (normalization underspecified)                          | 0      | Resolved in v2                                                         |
| v1 M2 (merge table gaps)                                      | 0      | Resolved in v2                                                         |
| v1 Minors (all 6)                                             | 0      | Resolved in v2                                                         |
| v2 B1 (tokenizer contradiction)                               | 0      | Resolved in v3                                                         |
| v2 M1 (Generic/None unceph-version; flag exclusion)           | 0      | Resolved in v3                                                         |
| v2 M2 (commit 2 oversized)                                    | 0      | Resolved in v3                                                         |
| v2 Minors (all 4)                                             | 0      | Resolved in v3                                                         |
| N1: §5 future note overstates base_ref feasibility            | -4     | Documentation inaccuracy — out of scope but misleading for future work |
| N2: Text/JSON asymmetry across 2c/4 unexplained               | -3     | Missing rationale — implementer gap                                    |
| N3: None-no-op vs Generic-error asymmetry unexplained         | -3     | Missing rationale                                                      |
| N4: Table caption inaccuracy; group-key rendering unspecified | -3     | Nits                                                                   |
| **Total**                                                     | **87** |                                                                        |

**Score: 87 / 100 — GO.**

All prior blockers and major concerns are resolved. The remaining deductions are
documentation precision issues, not specification correctness or architectural
flaws. None prevent implementation. The design is correct and implementable as
written.

---

## 6. Verified Strengths (unchanged from v1/v2)

These are confirmed correct and should be preserved through implementation:

- Separation premise (mutable annotations at distinct key prefix; import never
  touches annotations key space)
- `None` ⟹ unassessed, never Generic — and the invariant is now upheld by the
  `Generic --unceph-version` error
- Adjacent tagging on `Applicability`/`VersionSpec` — technically correct (unit
  and newtype variants cannot use internal tagging)
- Zero-migration backward compatibility (`get_annotations → Option`)
- Six-commit DAG with clean dependency boundaries and no dead seams
- `schema_version: u32` on `PatchAnnotations` (non-regenerable,
  operator-authored)
- Single-writer assumption documented alongside comparable notes in the codebase

---

## 7. Required Actions Before Implementation

**None block.**

Suggested (can be fixed in the design doc before commit 1, or in commit
annotations and comments at implementation time):

1. **[Minor N1]** Amend §5 future note to distinguish PR branch `base_ref` from
   a version string; note that a branch→line map is required.
2. **[Minor N2]** Add one sentence to §8 commit 2c explaining that the
   annotations-aware text line waits until commit 4 and why.
3. **[Minor N3]** Add one sentence to §5 explaining the asymmetric handling of
   `None --unceph-version` (no-op) vs `Generic --unceph-version` (error).
4. **[Minor N4]** Narrow the §5 table caption from "identical for import and
   annotate" to reflect that import only reaches a subset of rows.

**Proceed with commit 1.**
