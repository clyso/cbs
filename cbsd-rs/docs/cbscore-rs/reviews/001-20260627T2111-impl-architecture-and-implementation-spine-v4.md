# 001 — Implementation Review v4: M2 Build — C3 fix-round follow-up

Scope: the post-autosquash commit stack `7ce0ce4..HEAD` on branch
`wip/cbscore-rs` (parent/base of the C3 work `f0cb5bb`). This is the **v4
follow-up** to v3 (`001-20260627T1629-...-v3.md`, GO / confidence 60): it
verifies that the v3 findings S1, S2, M1–M5 were fixed correctly and completely,
that the autosquash produced clean atomic commits, and that nothing regressed.

Commits in scope (oldest→newest), with the fix folded into each:

- `7ce0ce4` C2a — plan progress row C2a Pending→Done (M2).
- `dd0674b` C2b — plan progress row C2b Pending→Done (M2).
- `a17553d` C3 git-ops — unchanged (reviewed in v3, context only).
- `1bafea1` C3 secrets — M4: `#[ignore]`d plain-SSH `git_url_for` live test.
- `2a8c9a4` C3 prepare — M1a: shared `builder/test_support::retry_spawn` (+
  `#[cfg(test)] mod test_support;`); prepare tests use it.
- `4a23952` C3 rpmbuild + wire — S1: `Builder::build()` success/cleanup-on-
  failure tests; M1b: rpmbuild uses the shared helper; M2: C3 row → Done.
- `6c02864` design — S2: new design 007 v2 with the patch-traversal note; v1
  pinned (`version`/`superseded-by` frontmatter + pointer).
- `505912a` cosign — M5: shared `debug_out_cb()` used by `prepare_builder` and
  `install_cosign`; cosign output now streams to the debug log.

Verification performed: read every changed file at HEAD; read each commit's diff
and confirmed the fix landed in the intended target commit; read the Python
sources (`builder/prepare.py`, `rpmbuild.py`, `utils/secrets/git.py`) against
the port; confirmed no `fixup!`/`squash!` commits remain in scope; ran
`cargo test -p cbscore --lib` (**98 passed / 0 failed / 3 ignored**, up from
v3's 96/0/2 — the +2 are the S1 tests, the +1 ignored is the M4 test) and
`cargo clippy -p cbscore --tests` (clean); grepped the changeset for
`allow(dead_code)`/`allow(unused)` (none).

---

## Executive Summary

The fix round is **substantively correct and well-targeted**. All four v3
deductions are addressed: the `Builder::build()` cleanup contract is now
exercised by two real tests on both paths (S1), the `retry_spawn` test helper is
genuinely deduplicated into a shared `#[cfg(test)]` module with no copies
remaining (M1), the previously-untested plain-SSH keyscan/alias path has an
honest `#[ignore]`d live test (M4), cosign output streams to the debug log via a
shared callback (M5), and the patch-traversal divergence is recorded in a
superseding design 007 v2 with coherent supersession frontmatter (S2). The
autosquash is clean: every fix is folded into the correct commit, no leftover
`fixup!` commits, no dead-code suppressions, and each commit remains atomic and
single-concern. Tests are green and clippy is clean at HEAD.

Two **minor, non-blocking** residuals remain — both quality nits, neither a
correctness or security issue:

1. The M5 fix's new `debug_out_cb()` (in `builder/mod.rs`) is behaviourally
   identical to the pre-existing `debug_log()` (in `builder/rpmbuild.rs`); the
   fix created a near-twin production helper instead of sharing one (D2).
2. The S2 design note overstates Python's strictness: it says Python raises
   `NotADirectoryError` on **any** stray non-`.patch` file, but Python's
   version-name guard prunes the realistic stray files (`README`, `series`,
   `.gitignore`) before `iterdir()` is ever called — it only raises for a stray
   file named **exactly** like a version selector (D8).

Recommendation: **GO.** Fold the two nits opportunistically; neither blocks C4a.

Confidence: **80 / 100** (v3 was 60; Δ +20). See the breakdown table below.

---

## Finding-by-finding verification

### S1 (was D5) — `Builder::build()` cleanup tested — RESOLVED

`4a23952` adds two container-independent tests in `builder/mod.rs`:

- `build_prepares_compiles_and_cleans_up_on_success` — clones a local `file://`
  repo, runs the component's `build_rpms.sh` (`touch $3/RPMS/...`), asserts the
  report lists the component with the `get_version.sh` long version and no S3
  path, then asserts the worktrees are gone.
- `build_cleans_up_worktrees_when_a_component_build_fails` — same fixture with a
  `build_rpms.sh` that `exit 1`s; asserts the error is a `Step` (not a
  `Command`) and the worktrees are gone.

I checked the three things the prompt flagged as suspect:

- **The `file://` fixture genuinely creates a worktree before the failure.**
  `init_source` builds a real repo on a `testref` branch; `get_version.sh`
  prints `1.2.3-build`, `deps.sh` exits 0, so `prepare_components` succeeds and
  a worktree is checked out **before** `build_rpms` runs the failing script. The
  cleanup assertion therefore exercises a real worktree removal, not a vacuous
  no-op.
- **`retry_transient` does NOT mask real failures.** It retries **only** on
  `BuilderError::Command` (a spawn failure — the multithreaded write-then-exec
  `ETXTBSY` race), and returns everything else (`Step`, `Ok`) immediately
  (`other => return other`). A genuine build failure surfaces as `Step` (the
  script ran and exited non-zero), which is passed straight through — in the
  success-path test it would panic via `.expect("build should succeed")`, and in
  the failure-path test it is asserted. This is materially safer than the shared
  `retry_spawn` (which retries on any error); the author correctly chose the
  discriminating variant for the failure path.
- **`count_files == 0` is a sound cleanup proxy.** A surviving worktree leaves
  regular files (`README`, the `.git` file) under `<scratch>/git/worktrees/`;
  `git worktree remove` deletes the leaf and leaves only the empty `<name>/`
  parent, which contributes zero files. So `count_files(worktrees) == 0` ⇔ no
  checked-out content survived, scoped to the worktrees dir (the mirror repo
  lives under `git/repos/`, uncounted). The proxy correctly distinguishes
  "cleaned" from "leaked".

`build()` runs cleanup unconditionally
(`let outcome = build_rpms_for(...); prepared.cleanup().await; outcome.map(...)`),
so both the success and failure paths clean up — exactly the guarantee v3 wanted
guarded. Closed.

### M1 (was D2) — `retry_spawn` deduplicated — RESOLVED

The shared `pub(crate) async fn retry_spawn` now lives once in
`builder/test_support.rs` (added in `2a8c9a4`), declared
`#[cfg(test)] mod test_support;` in `builder/mod.rs`. Both `prepare.rs` and
`rpmbuild.rs` import it; a tree-wide `rg` confirms exactly one definition and no
surviving local copies. The module is test-only, so there is no dead code in
non-test builds, and at its introducing commit its sole consumer (`prepare.rs`)
is present — `rpmbuild.rs` lands later (`4a23952`) already importing the shared
helper, so the golden-bisect rule holds (no orphan helper, no broken
intermediate).

Residual nuance (not a separate deduction): the S1 fix introduced a second retry
helper, `retry_transient`, local to the `mod.rs` tests. It is genuinely distinct
in semantics (retry-on-spawn-only vs retry-on-any) and return type, so it is
**not** a verbatim duplicate of `retry_spawn`; but it is a mild consolidation
opportunity — `retry_transient` is the safer primitive and could subsume
`retry_spawn`'s success-path uses. Test-only, low priority.

### M4 — plain-SSH keyscan/alias path covered — RESOLVED

`1bafea1` adds `plain_ssh_secret_materializes_a_key_and_alias` in
`utils/secrets/git.rs`, `#[ignore]`d with the reason "requires network and
ssh-keyscan". It drives the full `git_url_for` → `materialize_ssh` →
`ssh_keyscan` → `write_ssh_material` path and asserts the `<10 letters>:<repo>`
alias shape, the 0600 key, the `Host` alias in `config`, the scanned
`known_hosts`, and key removal when the resolved URL drops. It is faithful to
the code path: `materialize_ssh` parses host/port/path via the shared `http_*`
regex groups, which match an `https://` URL identically to `ssh://`, so using
`github.com` as the scan target exercises the real keyscan+alias assembly.
Correctly registered (`--ignored --list` shows it) and excluded from the default
run. The offline half (`write_ssh_material`) keeps its own non-ignored unit
test. Closed.

### M5 — cosign output streams to the debug log — RESOLVED (with a D2)

`505912a` extracts `debug_out_cb()` in `builder/mod.rs` and passes it as
`out_cb: Some(&log_cb)` from **both** `prepare_builder` (the toolchain steps)
and `install_cosign`. Cosign stdout now reaches the BUILDER debug target,
closing the v3 gap (Python's `logger.debug(stdout)`); the Rust streams
line-by-line, which is functionally equivalent (slightly more granular). No
borrow/lifetime issue: `log_cb` is bound once and outlives the loop in
`prepare_builder`; the closure captures nothing, so `Some(&log_cb)` borrows a
value with a sufficient lifetime — the suite compiles and passes.

**New finding (D2):** `debug_out_cb()` (mod.rs) is byte-for-byte equivalent in
behaviour to the pre-existing `debug_log()` (rpmbuild.rs) — both return
`impl Fn(String) -> OutLine` boxing `debug!(target: BUILDER, "{line}")`. The fix
added a third near-identical copy rather than hoisting one shared helper. Real
impact is negligible (a 3-line observability closure), but it is a clear
deduplication target and is flagged for symmetry with how v3 scored the
`retry_spawn` duplicate.

### S2 (was D8) — patch-traversal divergence documented — RESOLVED, note INACCURATE

`6c02864` supersedes design 007 with v2 (`007-20260627T2015-...-v2.md`) and adds
a "Patch traversal — stray files tolerated" fidelity note; v1 is pinned with
`version: 1` / `superseded-by: 2` frontmatter and a pointer, and v2 with
`version: 2` and a supersedes note. The supersession frontmatter is coherent and
cross-linked.

However, the note's characterisation of Python is **overbroad**. It states the
port skips a stray non-`.patch` file while "Python's `_get_patches_by_prio`
instead recurses into it and raises `NotADirectoryError`, failing the build."
Reading `prepare.py:135-162`: every recursive call begins with a version-name
guard — `if cur_prio > 0:` compares `path.name` against the exact / minor /
major version and **returns early** when none match. So a stray file whose name
does not match a version selector (`README`, `series`, `.gitignore`, `notes.txt`
— the realistic cases) is pruned by that early return and **never reaches
`iterdir()`**; Python raises **no** error and, like Rust, contributes nothing.
Python raises `NotADirectoryError` **only** for the narrow case of a stray
non-`.patch` file named exactly like a version selector (e.g. a file literally
named `1.2.3`). So in the common stray-file case there is **no** divergence at
all; the genuine divergence is the version-named-file edge. The note (which
faithfully implemented v3's own imprecise recommended wording) thus overstates
Python's strictness, and a reader who tests "drop a `README` in `patches/`"
would find Python does **not** fail, contradicting the note. −5 D8. The code
itself is fine; only the design note needs tightening.

### M2 (was D10) — plan progress tables — MOSTLY RESOLVED

The per-commit rows in `plans/001-20260623T1725-03-build.md` are flipped: C2a,
C2b, C3 → **Done** (landed in `7ce0ce4`, `dd0674b`, `4a23952` respectively). The
milestone rollup in `plans/README.md:25` still shows **M2 Pending**. That is
defensible: M2 spans C2–C6 (8 commits) and is genuinely incomplete (C4a–C6
pending), and the rollup vocabulary is binary Pending/Done with no "In
Progress", so "Pending" is the honest binary value. The substantive part of v3's
ask — per-commit tracking — is done. Noted, not deducted.

### M3 (transitional report) — carried forward

Unchanged and out of fix scope: `build_report` still derives from
`prepared.infos()` and lists components that produced no RPMs; the C3 commit
message and design both flag this as reworked on the release descriptor in
C4–C6. No action now.

---

## Autosquash integrity & commit hygiene

- **No leftover `fixup!`/`squash!` commits** in `7ce0ce4..HEAD` (confirmed via
  `git log`).
- **Every fix is in the correct target commit** (verified by diffing each
  commit): M2/C2a→`7ce0ce4`, M2/C2b→`dd0674b`, M4→`1bafea1`, M1a→`2a8c9a4`,
  S1+M1b+M2/C3→`4a23952`, S2→`6c02864`, M5→`505912a`. No fix landed in the wrong
  commit.
- **Golden bisect holds.** `test_support.rs` and its `mod test_support;`
  declaration co-land in `2a8c9a4`, with `prepare.rs` as the only consumer at
  that point (`rpmbuild.rs` arrives in the next commit already importing the
  shared helper). No commit introduces a helper without a same-commit reader; no
  `allow(dead_code)`/`allow(unused)` anywhere in the changeset.
- **The two standalone commits are justified.** `6c02864` is docs-only (a design
  supersession) and `505912a` is a small, self-contained behavioural tweak
  (cosign streaming) that did not belong inside a C-slice; both pass the smell
  test as independent, revertable units. Subject lines stay Ceph-style and ≤72
  chars.
- **Green at HEAD.** `cargo test -p cbscore --lib`: 98 passed / 0 failed / 3
  ignored; `cargo clippy -p cbscore --tests`: clean.

---

## New issues / regressions

- **D2 — `debug_out_cb` duplicates `debug_log`** (see M5). Minor, test-adjacent
  observability; dedup-able into one shared helper.
- **D8 — design 007 v2 patch-traversal note overstates Python** (see S2). Doc
  accuracy only; no code change.
- **Soft** — `retry_transient` is a near-cousin of the shared `retry_spawn` (see
  M1); consolidation opportunity, not separately deducted.
- No correctness, security, or data-integrity regressions. The redaction,
  RAII-key-cleanup, fail-fast fan-out, and always-cleanup invariants verified in
  v3 are untouched.

---

## Confidence Score

| Item                                                                       | Points | Description                                                                                               |
| -------------------------------------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------- |
| Starting score                                                             | 100    |                                                                                                           |
| D2: `debug_out_cb` (mod.rs) duplicates `debug_log` (rpmbuild.rs)           | -15    | M5 fix added a near-identical out_cb helper instead of sharing one. Trivial closure; low real impact.     |
| D8: design 007 v2 patch-traversal note overstates Python's failure surface | -5     | Claims any stray non-`.patch` file raises; Python's name guard only raises for a version-named file (S2). |
| **Total**                                                                  | **80** |                                                                                                           |

Retired from v3 (60 → 80, Δ +20): D5 (−15, S1 tests added), D2/`retry_spawn`
(−15, deduplicated), D10 (−5, per-commit rows flipped). The v3 D8 (−5,
divergence absent from the design) is replaced by a same-weight D8 of a
different character (the divergence is now documented, but inaccurately). The
sole new deduction is the M5-introduced D2.

Interpretation: 80 lands in the "acceptable with noted improvements" band. The
fixes are correct and complete; the two residuals are a trivial helper
duplication and a doc-accuracy nit, neither blocking.

---

## Verdict

**GO.** The C3 fix round correctly and completely addresses every v3 finding:
`Builder::build()` now has real success/cleanup-on-failure tests with a retry
helper that cannot mask failures, the `retry_spawn` helper is genuinely shared,
the plain-SSH path has an honest ignored live test, cosign output streams, and
the patch-traversal divergence is recorded in a coherent design supersession.
The autosquash is clean — every fix in its right commit, no leftover fixups, no
dead code, green tests, clean clippy. The two minor residuals do not block C4a.

Recommended before/with C4a (fold opportunistically):

1. **D2** — hoist one shared out_cb helper used by `debug_out_cb` and
   `debug_log`.
2. **D8** — tighten design 007 v2's patch-traversal note: Python prunes a
   non-version-named stray file via its name guard and raises only for a stray
   file named exactly like a version selector.
