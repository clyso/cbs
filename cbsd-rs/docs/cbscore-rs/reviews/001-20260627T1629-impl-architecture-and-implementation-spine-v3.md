# 001 — Implementation Review v3: M2 Build — C3 (secrets, prepare, rpmbuild)

Scope: commits `302b8a5..f07e6ed` on branch `wip/cbscore-rs` (parent `02d51c4`,
the C3 git-ops slice, already reviewed and out of scope except as context).

Three commits reviewed (oldest→newest):

- `302b8a5` — C3 slice 2: resolve git URLs against configured secrets
  (`utils/uris.rs`, `utils/redact.rs` `+SecureUrl`, `utils/secrets/*`).
- `a00e7b4` — C3 slice 3: prepare build components — clone, checkout, patch
  (`builder/prepare.rs`, `builder/mod.rs` error variants + `pub mod prepare`).
- `f07e6ed` — C3 slice 4: build component RPMs and wire the pipeline into
  `Builder::run` (`builder/rpmbuild.rs`, `builder/mod.rs` `run()` rewire).

Authoritative designs consulted: 001 (spine/invariants), 003
(subprocess/redaction), 004 (config/secrets/Vault), 007 (builder pipeline).
Plan: `plans/001-20260623T1725-03-build.md` (C3). Python source of truth:
`cbscore/builder/prepare.py`, `cbscore/builder/rpmbuild.py`,
`cbscore/builder/builder.py`, `cbscore/builder/utils.py`,
`cbscore/utils/secrets/git.py`, `cbscore/utils/secrets/utils.py`,
`cbscore/utils/uris.py`, `cbscore/versions/utils.py`.

Verification performed: read every changed file at HEAD and each commit's diff;
read the Python sources line-by-line against the port; `cargo check`,
`cargo clippy --tests`, and `cargo test --lib` for `cbscore` — clean build, **96
passed / 0 failed / 2 ignored** (the 2 ignored require podman + network).

---

## Executive Summary

This is a disciplined, faithful port of the first two builder stages. Design
fidelity to 003/004/007 is high: the longest-prefix URI matcher, the
credential-folding `SecureUrl` (structurally un-loggable), the RAII SSH-key
guard, the deepest-first patch selector, the sequential-deps / parallel-build
fan-out with abort-on-first-error, the `CES_CCACHE_PATH` pass-through, and the
worktree cleanup on both success and error paths are all present and behave as
the owning designs specify. The intricate `_get_patch_list` logic is ported
correctly and pinned by a golden test. The deliberate partials (vault-git → C4a)
are honestly stated in the commit messages and enforced in code with a typed
`VaultUnimplemented` error.

The v2 review's hard carryover requirement (**C1: remove derived `Debug` from
the four secret-value enums before C3 adds callers**) has been **resolved** —
`cbscore-types/src/secrets.rs:612-656` now hand-writes redacting `Debug` for
`GitSecret`/`StorageSecret`/`SigningSecret`/`RegistrySecret`, and C3's new
callers (`git_url_for` matching on `GitSecret`) inherit that protection.

There are **no correctness blockers and no security regressions**. The findings
are quality/process: (1) the `Builder::build()` orchestration — including the
emphasized "always clean up worktrees on the error path" guarantee — is
container-independent and therefore unit-testable, but has no test; (2) an
identical `retry_spawn` test helper is duplicated across two modules; (3) one
documented-in-code-but-not-in-design behavioral divergence in patch-tree
traversal; (4) the plan progress tables were not flipped after C2a/C2b/C3
landed. The commit split leans layer-wise but is defensible under the hard size
budget and each commit is independently green.

Recommendation: **approve with conditions** — add the `build()` cleanup-path
test and update the plan tables before proceeding to C4a; fold the remaining
items as cleanups.

---

## Critical Issues

None. No correctness, security, or data-integrity blocker was found. All
critical redaction paths are tested (`SecureUrl` censors password/token; a
`CmdArg::Secure` cannot leak through any formatting trait), the SSH key is
written 0600 and removed on guard `Drop`, and the fail-fast fan-out cleans up
finished siblings' worktrees.

---

## Significant Concerns

### S1 — `Builder::build()` orchestration is untested though readily testable (D5)

**Problem.** `f07e6ed`'s headline guarantee is that `Builder::build()` "always
cleans up the worktrees — on both the success and the error path"
(`builder/mod.rs:204-231`). That branch is exercised by **no test**. Crucially,
`build()` does **not** call `prepare_builder()` (only `run()` does), so
`build()` is fully container-independent: it does secrets assembly →
`load_components` → `prepare_components` (clones a local `file://` repo) →
`build_rpms_for` (runs local scripts) → `prepared.cleanup()`. Everything it
touches is already unit-tested in isolation (`prepare_components` and
`build_rpms` both have local tests), so a test that drives `build()` to a
`build_rpms` failure (e.g. a component whose `build_rpms.sh` exits non-zero) and
asserts the worktrees were removed is straightforward to write.

**Impact.** The cleanup-on-error contract — the most failure-prone part of the
commit and the one most likely to regress when C4b inserts sign/upload between
`build_rpms` and cleanup — has no regression guard. The `build_rpms_for` glue
(rpms-dir creation, el-version threading, ccache wiring) is likewise unexercised
end-to-end.

**Recommendation.** Add a `build()` test (no container needed) covering: (a) the
success path returns a report listing the prepared components and the worktrees
are gone afterward; (b) a `build_rpms` failure still removes the worktrees and
propagates the error. This is the one finding I'd want closed before C4a builds
on top of `build()`.

### S2 — `collect_patches` silently skips stray files where Python fails the build (D8)

**Problem.** Python's `_get_patches_by_prio` (`prepare.py:153-162`) recurses
into **every** non-`.patch` entry, including regular files — calling
`path.iterdir()` on a plain file raises `NotADirectoryError`, which
`_apply_patches` turns into a `BuilderError` and fails the build. The Rust
`collect_patches` (`prepare.rs:382-402`) recurses only into directories and
silently ignores a stray non-`.patch` plain file (e.g. a `README`, `.gitignore`,
or `series` file dropped in `patches/`). This is a real behavioral divergence:
such a tree fails in Python and succeeds in Rust.

**Impact.** Low in practice (patch trees normally hold only `.patch` files and
version subdirectories), and the Rust behavior is arguably the better one. But
it is an unflagged-at-design-level deviation: the code comment
(`prepare.rs:398-402`) explains it, yet design 007's "Fidelity notes" list only
the omission of Python's dead `patches_lst` block, not this traversal change.

**Recommendation.** Add a one-line entry to design 007's Fidelity notes ("a
stray non-`.patch` file under `patches/` is skipped, not fatal — Python would
raise"). No code change required; just close the design/code gap so the next
reader does not flag it as a port bug.

---

## Minor Findings

### M1 — `retry_spawn` test helper duplicated verbatim (D2)

The ~18-line `retry_spawn` ETXTBSY-absorbing helper is byte-identical in
`builder/prepare.rs:473-489` and `builder/rpmbuild.rs:357-373` (same doc
comment, same body). It should live once in a shared `#[cfg(test)]` util (e.g. a
small `builder::test_support` module) and be imported by both. Test-only, so low
real-world impact, but it is a clear deduplication target.

### M2 — Plan progress tables not updated after C2a/C2b/C3 (D10)

`cbsd-rs/CLAUDE.md` and `plans/README.md` both require flipping the progress
tables after each commit lands. `plans/001-20260623T1725-03-build.md` still
marks Commit 1 (C2a), Commit 2 (C2b), and Commit 3 (C3) as **Pending**, and the
README status table still shows M2 **Pending**, although C2a/C2b are landed (see
the parent history) and C3 is the work under review. Flip these to keep the plan
the source of truth.

### M3 — `build_report` lists components that produced no RPMs (transitional)

`Builder::build_report` (`mod.rs:264-283`) derives `ComponentReport`s from
`prepared.infos()`, so it includes components that `build_rpms` skipped (no
`rpm` build section). Python's final report derives from the **release
descriptor** (post-upload), which would drop them. The commit message
acknowledges this is a C3 placeholder reworked in C4–C6. No action now; flagged
so it is not lost when the report is rebuilt on the release descriptor in C4b.

### M4 — `materialize_ssh` keyscan/alias path is not covered

Only the offline `write_ssh_material` half of the plain-SSH branch is tested
(`git.rs:451-498`); `materialize_ssh` → `ssh_keyscan` → alias assembly has no
test because `ssh-keyscan` needs the network. Acceptable gap, but worth a note:
the plain-SSH end-to-end resolution is unexercised, so a regression in host/port
extraction or the `<alias>:<repo>` shape would not be caught.

### M5 — cosign stdout no longer debug-logged

Python logs cosign's stdout (`prepare.py:116 logger.debug(stdout)`); the Rust
`install_cosign` (`mod.rs:321-346`) runs with no `out_cb` and does not log
stdout. Error paths still carry stderr in `BuilderError::Step`, so this is a
negligible observability nit, not a D9. (This is C2b code touched only
incidentally by `f07e6ed`'s `run()` rewire.)

---

## Positive Divergences (intentional, aligned with design)

- **Worktree-leak fix on prepare failure.** Python's `prepare_components`
  `finally` only cleans up when `comp_infos` is truthy, which it is **not** on a
  fan-out failure — so successfully-finished siblings' worktrees leak. The Rust
  fail-fast path calls `cleanup_worktrees(&infos)` on the finished siblings
  (`prepare.rs:132-143`), matching design 007's "(or any failure) it removes the
  worktrees." A genuine improvement, consistent with the design's stated intent.
- **Deterministic patch directory ordering.** `read_dir_sorted`
  (`prepare.rs:408-422`) sorts directory entries by name; Python relies on
  unspecified `iterdir()` order. For two same-depth patches sharing a numeric
  prefix the resulting tie-order is now deterministic rather than
  filesystem-dependent. Strictly better; documented in code.
- **`matches_uri` is infallible by construction.** The port replaces Python's
  raw-regex-interpolation prefix check (which mishandles regex metacharacters in
  a path and can raise an "empty remainder" `URIError`) with segment-list prefix
  matching (`uris.rs`), eliminating that error path. Documented divergence with
  a golden test mirroring Python's case table.
- **v2 carryover C1 resolved.** Redacting hand-written `Debug` on all four
  secret-value enums now backs invariant 4 at the type level, exactly as the v2
  review required before C3 added callers.

---

## Commit Hygiene

The plan defined C3 as a **single** capability commit ("build compiles a
component's RPMs"). At ~2,660 inserted lines it is roughly 3× the ~800-line
budget, so a split was mandatory. The implementer split it four ways (including
the parent git-ops slice): git tool wrappers → secrets resolution → prepare
stage → rpmbuild + `run()` wiring.

This leans **layer-wise**, and two of the in-scope commits land foundation/stage
code whose first **production** consumer is in a later commit:

- `302b8a5` (secrets) has no production caller — `git_url_for`'s first consumer,
  `prepare_components`, lands in the next commit.
- `a00e7b4` (prepare) has no production caller — verified that `run()` at that
  commit still returns `self.skipped_report()`; `prepare_components` is wired in
  only at `f07e6ed`.

That ordering (foundation before consumer) is the pattern the project's own
conventions caution against ("foundational code lands in the commit of its first
consumer"). **However, I do not score this as a commit-boundary violation
(D12):** each of the three commits independently passes the five-point smell
test — it compiles alone, carries **no dead code** (every added `pub` item has a
real test reader in the same commit; no `#[allow(dead_code)]`/`unused`
suppression exists anywhere in the changeset, verified by grep), addresses a
single concern, and is safely revertable. The split also fits the git-commits
skill's allowed "Library + consumer" seam, whose sole precondition — each half
delivers _independently testable_ functionality — is met with real, green tests
(not "tests later"). The commit messages are honest about every partial. Net: a
defensible split given the size constraint, with a layer-leaning smell worth
noting but not deducting.

Subject lines are all ≤72 chars, Ceph-style, DCO-signed, with exactly one
`Co-authored-by` trailer — conforming to `cbsd-rs/CLAUDE.md`.

---

## Design & Fidelity Conformance

| Area                                          | Verdict | Notes                                                                                                                            |
| --------------------------------------------- | ------- | -------------------------------------------------------------------------------------------------------------------------------- |
| 004 longest-prefix URI match                  | Pass    | `find_best_secret_candidate` + `matches_uri` golden cases ported from Python's tables; tie-break by remainder depth, first-wins. |
| 004 `git_url_for` plain/no-match              | Pass    | https/token fold into `SecureUrl`; ssh materialises 0600 key + alias with RAII drop; no-match = URL unchanged.                   |
| 004 vault-git deferral to C4a                 | Pass    | `VaultUnimplemented` typed error + test; commit message states the partial (review S-2 requirement met).                         |
| 003 `SecureUrl` redaction (invariant 4)       | Pass    | Single-pass template render; censored in `redacted()`/`Debug`/`Display`; plaintext only via `plaintext()`.                       |
| 007 prepare: clone→checkout→patch→record      | Pass    | `BuildComponentInfo` fields match; worktree removed on finalize failure (prepare.py:379 parity).                                 |
| 007 `_get_patch_list` selection               | Pass    | Deepest-first, numeric-prefix order, exact/minor(M.m.p)/major(M.m) dir match; golden test; matches `versions/utils.py`.          |
| 007 rpmbuild: seq deps / parallel build       | Pass    | `install_deps` sequential (BTreeMap order, tested); per-component topdir; fail-fast `JoinSet`.                                   |
| 007 `skip_build` / ccache / `CES_CCACHE_PATH` | Pass    | Topdir laid out, script not run; ccache made absolute via `abs_lenient`; all three tested.                                       |
| 007 always-cleanup worktrees                  | Pass\*  | Correct in code (both paths); \*untested — see S1.                                                                               |
| 007 dead-code omission                        | Pass    | Python's unused `patches_lst` block correctly not ported.                                                                        |
| 007 patch-tree traversal                      | Deviate | Stray non-`.patch` file skipped vs Python raise — documented in code, not in design (S2).                                        |
| `reset_python_env` dropped                    | Pass    | No venv on `cbsbuild` PATH; build/deps scripts inherit clean env (001/003).                                                      |

---

## Confidence Score

| Item                                                                                                      | Points | Description                                                                                 |
| --------------------------------------------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------- |
| Starting score                                                                                            | 100    |                                                                                             |
| D5: `Builder::build()` cleanup-on-error path untested (container-independent, readily testable)           | -15    | The emphasized always-cleanup guarantee + rpms/ccache wiring have no test (S1).             |
| D2: `retry_spawn` duplicated verbatim across `prepare.rs` and `rpmbuild.rs` tests                         | -15    | Identical 18-line helper; belongs in one shared test util (M1). Test-only, low real impact. |
| D8: `collect_patches` skips stray files where Python fails — divergence absent from design fidelity notes | -5     | Documented in code only (S2).                                                               |
| D10: plan progress tables not updated after C2a/C2b/C3 landed                                             | -5     | Violates explicit `CLAUDE.md`/`plans/README.md` rule (M2).                                  |
| **Total**                                                                                                 | **60** |                                                                                             |

Interpretation: 60 lands in the "significant issues" band, but the band is
driven almost entirely by two −15 hygiene items (an untested-but-easy glue path
and a test-helper duplication), not by any correctness or security defect. The
ported behavior itself is faithful and green.

---

## Verdict

**GO (approve with conditions).** C3 delivers its capability — the build
resolves secrets, prepares component sources (clone/checkout/patch), and
compiles their RPMs, wired into `Builder::run` — faithfully to designs
003/004/007 and the C3 plan, with no correctness or security blocker and a fully
green test run. The v2 carryover requirement is resolved.

Required before C4a:

1. **S1 / D5** — add the container-independent `Builder::build()` test covering
   the success path and the cleanup-on-`build_rpms`-failure path.
2. **M2 / D10** — flip the C2a/C2b/C3 rows in the plan progress tables.

Recommended cleanups (fold opportunistically):

1. **M1 / D2** — hoist `retry_spawn` into a shared test util.
2. **S2 / D8** — add the patch-traversal divergence to design 007's Fidelity
   notes.
