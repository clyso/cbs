# Review — Plan: `patch list` and `patch info` (v1)

- **Reviewing:**
  [`../plans/002-20260627T1659-patch-list-info.md`](../plans/002-20260627T1659-patch-list-info.md)
- **Against design:**
  [`../design/002-20260627T1645-patch-list-info.md`](../design/002-20260627T1645-patch-list-info.md)
- **And against:** the actual `crt` / `crt-store` / `crt-core` source on
  `wip/release-tool-v2`.
- **Date:** 2026-06-27
- **Type:** plan review
- **Verdict:** **GO with minor corrections** (confidence 84/100)

## What was reviewed

The plan adds two read-only CLI commands — `crt patch list` and `crt patch info`
(`--json` on both; full-hash or unique-short-prefix lookup for `info`) — plus
one new store enumerator, `list_patches`. It proposes two commits (`list`, then
`info`). I verified every load-bearing claim against the source rather than
trusting the plan or the design.

## Summary

The plan is well-grounded. Its core technical claims hold against the code:

- The meta key really is `patches/meta/sha256/<hex>.json` (4 parts)
  (`crt-store/src/lib.rs:107-109`), so `parse_meta_hash` mirroring
  `parse_release_key`'s 4-part check (`lib.rs:157-168`) is sound.
- `object_store::list` is **segment-aware**, not a raw string prefix
  (`object_store-0.13.2/src/path/mod.rs:349-355`; tests at `:604-654`). Listing
  `patches/meta/sha256` cannot bleed into `patches/blobs/sha256` or a
  hypothetical `…/sha256extra` sibling. No trailing-slash bug.
- `try_collect` is in scope in `crt-store` (`futures = "0.3"`,
  `crt-store/Cargo.toml:17`; `use futures::TryStreamExt;` at `lib.rs:16`), and
  `list_patches` lives in `crt-store` — so the
  `list(...) + try_collect() + filter_map` shape compiles there. (`crt` itself
  has no `futures` dep, but never needs one — it calls `list_patches`, not the
  stream.)
- `PatchMeta` fields match `render_info`'s block exactly, and the struct derives
  `Serialize` (`crt-core/src/meta.rs:50-66`), so `--json` and the on-disk record
  are the same shape — the design's "faithful to the stored record" claim holds
  (`put_meta` serializes the identical struct, `lib.rs:272-278`).
- The equivalence cross-reference logic is correct given how `import` populates
  the index: `put_patch_id` is written only on the first blob for a `patch_id`
  (`import.rs:277-279`), so `get_patch_id` returns the representative; the
  `!= meta.blob_hash` filter reproduces `import.rs:256` exactly.
- The CLI data-fn + `render_*`/`--json` split matches `ReleaseCmd::{List,Info}`
  (`main.rs:429-438`; `release::list_releases` returns data,
  `release::show_info` returns a `String`), and `open_store` is the established
  pattern (`main.rs:248-268`, used by `PatchCmd::Import`).
- Seq `002`, filename, and component routing follow the convention; no `002`
  review pre-exists.

The findings below are a one-line sort correction (a design↔plan wording
tension) plus three edge-case tightenings in `patch info`'s argument parsing.
None threatens the design; all are cheap to fix before implementation. Commit
boundaries are sound.

## Findings (by severity)

### Important

**F1 — `(subject, blob_hash)` sort cannot compile as the design states; the plan
half-fixes it.** (design:90-91, plan:44, plan:48)

`Sha256` derives only `Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`
(`crt-core/src/lib.rs:59`) — **no `Ord`/`PartialOrd`**. A literal sort key of
`(subject, blob_hash)` where `blob_hash: Sha256` will not compile. The design at
line 90 states exactly that tuple and at line 91 argues a separate hash "is
unnecessary," which reads as an instruction to sort on the `Sha256` directly —
which is the broken form.

The plan body at **line 44** correctly writes "sort by
`(subject, blob_hash hex)`", i.e. `(subject, blob_hash.to_hex())` — the
compilable form. But plan **lines 48 and 168** (the test bullets) and the design
both restate the bare `(subject, blob_hash)`. This is an internal inconsistency:
an implementer reading the design or the test bullets first may write the
non-compiling key.

The compiler catches this immediately, so it is not a no-go — but the plan
should state the sort key as `(subject, blob_hash.to_hex())` (or equivalent) in
**one** place and make the design's "separate hash unnecessary" remark
consistent with it, so the intent is unambiguous. Note the existing
`release::list_releases` sorts on `&String` tuples (`release.rs:416-418`), so
there is no precedent in the tree for an `Ord` on `Sha256` to lean on.

_Fix:_ specify
`patches.sort_by(|a, b| (&a.subject, a.blob_hash.to_hex()).cmp(&(&b.subject, b.blob_hash.to_hex())))`
(or a `sort_by_key` with an owned `(String, String)`), and reconcile
design:90-91.

**F2 — lowercase-hex rejection is net-new logic the plan hand-waves, with an
order dependency.** (plan:80-81, design:116-117)

The plan says "Reject a non-lowercase-hex arg" but there is **no existing
lowercase-hex predicate** in the tree, and `Sha256::try_from` validates only the
_full_ 64-char form (`crt-core/src/lib.rs:91-101`) — it is no help for a prefix.
Two concrete traps the plan must address explicitly:

1. `char::is_ascii_hexdigit()` accepts **uppercase** `A-F`. To honor the
   design's "lowercase hex" rule the implementer needs an explicit predicate
   (`c.is_ascii_digit() || ('a'..='f').contains(&c)`), not the stdlib helper.
2. **Order matters:** the hex check must run _before_ the length-based
   full/prefix split. The design (design:116-117) puts hex-validation first
   ("the arg must be lowercase hex (else a clear error), then:"). The plan's
   bullet order (plan:80-86) lists the `<4` and non-hex rejection first, which
   is consistent — but the plan should state plainly that a 64-char _non-hex_
   arg must produce the "not hex" error, not fall through to `Sha256::try_from`
   and surface that crate's `InvalidSha256` message. Keep hex-first to avoid a
   confusing error on a 64-char typo.

_Fix:_ name the predicate and assert the hex-before-length ordering in the plan.

### Minor

**F3 — `len > 64` is unspecified.** (plan:80-86, design:114-123)

The plan enumerates exactly two length classes: full (`len == 64`) and prefix
(`len 4-63`). A 65+-char hex arg is unhandled. Depending on how the `if/else` is
written it will either fall into the prefix branch (enumerate → 0 matches →
not-found, which is acceptable) or hit a slicing/range assumption. The design
has the same gap (design:114-123). Cheap to close: state that anything longer
than 64 is rejected (or folded into the not-found path) so the behavior is
intentional, not incidental.

**F4 — equivalence cross-reference is one-way (limitation, not a bug).**
(design:142-145, plan:90-91)

Because `get_patch_id` returns the _first-seen_ representative blob
(`import.rs:277-279`), `patch info <non-representative-hash>` shows the
equivalence line, but `patch info <representative-hash>` shows _no_ line even
when other equivalent blobs exist (the filter `!= meta.blob_hash` removes the
self match and there is no reverse index). This mirrors `import`'s own one-way
behavior (`import.rs:256`) and the design labels it a "nice-to-have," so it is
acceptable — but the plan/design should note the asymmetry so it is not later
mistaken for a defect. No code change required.

**F5 — `list`'s O(N) `get_meta` fan-out is acknowledged but the failure mode is
not.** (design:92-95, plan:43)

The design documents the 1 list + N gets cost (design:92-95). Worth one extra
sentence in the plan: a meta object that fails to deserialize (or a blob whose
meta is missing — `import` writes blob-then-meta and a partial failure can leave
a blob without meta, per `import.rs:270-275`) will abort the whole `list` via
`?`. For a read-only enumerator that is defensible, but the plan should say
whether a single corrupt/missing meta should fail the listing or be skipped, so
the behavior is a decision rather than an accident. (`list_patches` enumerates
the _meta_ prefix, so a blob-without-meta is invisible to it — only a corrupt
meta JSON is the live risk. Minor.)

## Commit boundaries (git-commits smell test)

The two-commit split is **justified** — do not merge or re-split:

| Commit                             | Capability          | Caller for new code?                                                                   | Smell test                                                             |
| ---------------------------------- | ------------------- | -------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| 1 `crt: list imported patches`     | enumerate the store | `list_patches` ships with its first caller (`patch::list`) — no dead-code intermediate | one sentence; compiles; revertable; testable; no `#[allow(dead_code)]` |
| 2 `crt: inspect an imported patch` | inspect one patch   | adds a _second_ caller of `list_patches` for prefix resolution; no new store change    | passes alone; distinct capability, not a move/rename                   |

- Each commit delivers a **capability**, not a layer (the new store method lands
  _with_ its caller in commit 1 — the git-commits anti-pattern of "DB layer
  first, callers later" is avoided).
- Sizing is ~150 / ~130 LOC — both under the 200 floor individually, but each is
  a meaningful standalone capability (enumerate vs. inspect), so the floor's
  "question whether it stands alone" test passes. A single combined commit (~280
  LOC) would _also_ be defensible (well under 800) — that is a preference, not a
  defect; the proposed split is the cleaner of the two.
- Ordering (`list` → `info`) is correct: `info`'s prefix path reuses
  `list_patches`, so landing it first keeps the only `crt-store` change isolated
  in commit 1.

Commit messages follow the `component: imperative` form, are within the subject
budget, and describe intent. Good.

## Design ↔ plan consistency

Faithful overall: function names (`list_patches`, `patch::list`, `patch::find`,
`render_list`, `render_info`), return types, output streams (stdout result /
stderr summary), `--json` suppressing the stderr summary, and the short-hash
rules all match between the two documents and the existing CLI idioms. The only
drift is **F1** (the bare `(subject, blob_hash)` sort tuple, restated in the
design and in the plan's test bullets, contradicting the plan's own line 44
"hex" form) and the shared **F3** `len > 64` gap.

## Confidence score

| Item                                                                                 | Points | Description                                                                                         |
| ------------------------------------------------------------------------------------ | ------ | --------------------------------------------------------------------------------------------------- |
| Starting score                                                                       | 100    |                                                                                                     |
| F1 (D8): sort key cannot compile as the design states; plan inconsistent with itself | -8     | Real compile error in the literal form; plan:44 half-corrects it; one-line fix, compiler-caught     |
| F2 (D8): lowercase-hex rejection unspecified + order dependency                      | -5     | Net-new predicate; `is_ascii_hexdigit` accepts uppercase; hex-before-length ordering must be stated |
| F3 (D8): `len > 64` argument class unhandled                                         | -2     | Edge case omitted in both plan and design                                                           |
| F4 (D11): one-way equivalence asymmetry undocumented                                 | -1     | Acceptable limitation; should be noted, not fixed                                                   |
| F5 (D9): `list` fan-out failure mode (corrupt meta aborts) undecided                 | -? (0) | Documented cost; failure behavior should be a stated decision                                       |
| **Total**                                                                            | **84** |                                                                                                     |

(Deductions are scaled to a _plan_ review: the design is sound and every finding
is a cheap pre-implementation correction, so each is weighted toward the low end
of its criterion.)

**Interpretation: 84 — acceptable with noted improvements; fix F1 and F2 before
implementing.**

## Recommendation

**GO**, conditional on three plan edits before coding:

1. **F1:** state the sort key as `(subject, blob_hash.to_hex())` in one place
   and reconcile design:90-91 — the bare `Sha256` tuple does not compile.
2. **F2:** name an explicit lowercase-hex predicate (not `is_ascii_hexdigit`)
   and assert the hex-check-before-length-split ordering.
3. **F3:** state explicit handling for `len > 64` (reject or fold into
   not-found).

F4 and F5 are documentation/decision notes, not blockers. Commit boundaries,
store surface, types, CLI parity, prefix-boundary safety, and `--json` fidelity
are all confirmed correct against the source — no rework needed there.
