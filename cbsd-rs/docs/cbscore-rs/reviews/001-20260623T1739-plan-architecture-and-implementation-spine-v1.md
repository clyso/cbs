# Plan-corpus review — cbscore-rs implementation plans (M0–M4) v1

Adversarial review of the implementation-plan corpus for the Rust port of the
Python `cbscore` build library. Reviewed as a plan (type=plan), shared seq 001
with the architecture-and-implementation spine (`design/001`).

**Reviewed artifacts**

- `plans/README.md`
- `plans/001-20260623T1725-01-bootstrap.md` (M0/C0)
- `plans/001-20260623T1725-02-versions-create.md` (M1/C1)
- `plans/001-20260623T1725-03-build.md` (M2/C2–C6)
- `plans/001-20260623T1725-04-versions-list.md` (M3/C7)
- `plans/001-20260623T1725-05-worker.md` (M4/C8)

**Verified against** the design corpus (`design/001`–`011`, the contract), the
top-level `README.md`/`ROADMAP.md`/`CLAUDE.md`, the whole-set design review
(`reviews/000-…-v1.md`, verdict GO), and the **actual current workspace**:
`cbsd-rs/Cargo.toml`, `container/ContainerFile.cbsd-rs`,
`.github/workflows/release-cbsd-rs.yaml`, the worker cutover surface
(`cbsd-worker/src/build/{executor,output,supervisor}.rs`, `ws/handler.rs`,
`cbsd-proto/src/ws.rs`), `scripts/cbscore-wrapper.py`, and the Python source
under `cbscore/src/cbscore/`.

---

## Executive summary

This is a strong, design-faithful plan corpus. The capability-not-layer
discipline is real and well executed: every milestone leads with an
operator/worker-visible capability, foundational code is consistently scheduled
into its first-consumer commit, and the plans even flag their own sizing and
seam risks honestly in "Notes for the plan-review" blocks. Fidelity to the
owning designs is high — I cross-checked the named types, functions, and
contracts against 002–011 and the ground-truth code, and found the plans
accurate on the load-bearing details (the four `exec.kill()` sites, the
register-then-attach order, `MAX_REPORT_SIZE = 65_536`, `BuildFinishedStatus`,
the `list_releases` arity bug, the parse parametric cases, the fix-don't-
reproduce placements). The single most important finding is **not** in the Rust
design — it is an infrastructure-reality gap: **the M0/C2 musl de-risk proof is
wired into a release-only CI workflow, so it will not run "before any subsystem
work begins" as both the plan and `design/001` promise** (Critical Issue 1). The
second material finding is that **M4/C8 Commit 2's "drop python3" cannot be
satisfied by editing the worker image stage alone** — python is the base image
of a shared `worker-base` stage (Critical Issue 2). Everything else is a
significant-or-minor sizing/sequencing/wording concern. With C0's CI gating
corrected and M4/C8's image-cleanup scope corrected, this corpus is ready to
drive implementation milestone by milestone.

Verdict: **Approve with conditions.**

---

## Critical issues 🔴

### C-1 — The M0/C2 musl de-risk proof runs only at release time, not before subsystem work

**Problem.** `design/001` (Build target, and the C0 row of the commit map) and
M0/C0 Commit 2 both state the `aws-sdk-s3` + `vaultrs` static-musl linkage proof
is "proven in CI **before any subsystem work begins**." The plan says to add the
probe job to `.github/workflows/release-cbsd-rs.yaml`. But that is the **only**
workflow in the repo, and it triggers **exclusively** on `push: tags: v*` and
`workflow_dispatch` (verified: `release-cbsd-rs.yaml:6-16`). There is no
PR/branch CI for the Rust workspace at all. A probe job added there fires only
when someone cuts a release tag — i.e. **after** M1–M4 have already been built,
not before. The de-risk spike's entire purpose (catch a musl link failure in
`aws-sdk-s3`/`vaultrs`/their crypto stack before the port commits to them at
C4/C6) is defeated if the gate runs months later at release time.

**Impact.** The B1 blocker the spike exists to retire stays effectively un-
retired during the at-risk window. If `vaultrs` or `aws-sdk-s3`'s TLS/crypto
provider does not link static-musl, the team discovers it at C4 (deep into M2)
or at the first release tag — exactly the late, expensive failure C0 is meant to
prevent. The plan's own acceptance gate ("`cargo build --workspace` still does
not depend on aws-sdk-s3/vaultrs … verified by `cargo tree`") also never runs
pre-merge.

**Recommendation.** C0 Commit 2 must either (a) add a **new** push/PR-triggered
CI workflow (e.g. `.github/workflows/ci-cbsd-rs.yaml` on `pull_request` + `push`
to the working branch) that runs the musl-probe build/run and the `cargo tree`
absence check, or (b) explicitly state the probe is a **local acceptance gate
run by the implementer on the C0 branch** and recorded in the commit message,
with the CI entry as a secondary safety net. Option (a) is strongly preferred —
it is the only way "proven in CI before subsystem work" is true. The plan text
"the existing matrix for `cbc`" misleads: that matrix is release-gated, so
reusing it does not deliver a pre-work gate. This is a doc/ plan correction plus
one CI file, not a design change.

### C-2 — M4/C8 Commit 2 "drop python3" understates the real ContainerFile surface

**Problem.** M4/C8 Commit 2 says: "`container/ContainerFile.cbsd-rs` — drop the
worker image's `python3`, the installed `cbscore`, and the `cbscore-wrapper.py`
COPY." In the real ContainerFile, the production worker stage `cbsd-rs-worker`
is `FROM worker-base` (line 156), and `worker-base` is itself
`FROM python:3.13-alpine3.21` (line 46) with `uv` + `cbscore` installed in it
(lines 54-83). `worker-base` is **shared by the dev worker too**
(`cbsd-rs-dev-worker`, line 216). So:

- "Dropping python3" is not a line-deletion in `cbsd-rs-worker`; python is the
  **base image** of a shared stage. Removing it means either re-pointing
  `cbsd-rs-worker` at a non-python base (`alpine:3.21`) — which forks it away
  from `worker-base` and the dev worker — or rebuilding the stage topology.
- The `cbscore`/`uv` install (lines 62-83) and `CBSCORE_PATH` env (line 83) live
  in `worker-base`, not in `cbsd-rs-worker`; the dev worker inherits them.
- `cbscore-wrapper.py` is COPY'd at line 159 in `cbsd-rs-worker` (that part of
  the plan is correct).

**Impact.** As written, Commit 2 is under-scoped and would either (a) leave
python3 in the production image (the base image still ships it) — failing the
"worker image builds without Python" testable — or (b) be implemented as a
larger stage-topology change than the plan budgets for, with collateral impact
on the dev worker image. An implementer following the plan literally produces a
non-functional cleanup or an unplanned-size diff.

**Recommendation.** Rewrite Commit 2 to name the real surface: re-point the
production worker stage at a python-free base (e.g. introduce a slim
`worker-base-rs` `FROM alpine:3.21` with just `podman`/`ca-certs`/`curl`, used
by `cbsd-rs-worker`), and decide the dev worker's fate explicitly (it may keep
python for cargo-watch convenience, or also move). Either keep the cbsbuild
`COPY` from C8 Commit 1 in the new base, or carry it. Re-budget the commit. The
M4 sizing note already flags merging Commit 2 into Commit 1; whichever way it
lands, the ContainerFile reality (shared python base stage) must be in the plan
text so the cleanup is actually achievable.

---

## Significant concerns 🟡

### S-1 — M2/C4 is acknowledged-oversized; the recommended split must be the committed plan, not a maybe

**Problem.** M2 Commit 4 (C4) bundles: Vault client + SecretsMgr-complete (vault
resolution for every family, including the C3 vault-git variants) + S3
write-path client + GPG signing + createrepo + RPM upload + release-descriptor
write. The plan's own note says this "is well over ~800 lines" and "strongly
consider splitting" into C4a (Vault + SecretsMgr resolution + S3 write) and C4b
(GPG signing + createrepo + upload + releases write). Each half is independently
testable (C4a via MinIO/Vault round-trips per 005's H4 gate; C4b via a
sign+upload run). This is correct — but the corpus still lists C4 as one commit
in the progress table and the README total ("14 commits"), leaving the split as
an open question rather than the plan.

**Impact.** A single C4 commit is a near-certain >800-line, multi-concern commit
spanning two new external dependencies (Vault, S3) plus two build stages — it
fails the git-commits smell test (one-sentence purpose; revertable without
collateral). Leaving it nominally-one invites it to be built as one.

**Recommendation.** Promote the C4a/C4b split into the committed breakdown now:
update M2's progress table and the README count to 8 commits in M2 (15 total).
The split is capability-clean: C4a = "build resolves Vault-backed secrets and
can read/write S3 objects" (first Vault + first S3 consumer landing with the
S3-client tests); C4b = "build signs RPMs and uploads them + release descriptors
to S3." Note that C4a is where the C3-deferred **vault-git** `git_url_for`
variants complete (see S-2), which gives C4a a concrete first consumer for the
Vault client beyond the test.

### S-2 — The C3→C4 vault-git seam leaves `git_url_for` partial; confirm C3 is functional for the real fan-out, not just public repos

**Problem.** M2/C3 lands `SecretsMgr.git_url_for` with only the **plain** ssh /
https / token / no-match cases; the **vault-backed** git resolution defers to C4
(needs the Vault client). 004 specifies `git_url_for` as longest-prefix with
both plain and vault variants. The plan's reasoning (plain + no-match covers
public and plain-cred repos; Vault is "first consumer = C4" per 001) is sound,
**but** C3's capability is "build compiles a component's RPMs," which runs
`prepare_components` → `git_clone` over the descriptor's real component repos.
If any component in a realistic C3 test descriptor resolves to a **vault** git
secret, `git_url_for` has no path for it at C3.

**Impact.** Low-to-moderate: C3 is functional for public and plain-cred repos
(the common Ceph case clones over https/ssh with plain or no creds), so the
capability is genuinely deliverable and testable at C3. The risk is only that a
C3 acceptance test against a vault-git-authenticated component would fail until
C4. This is a documentation/contract clarity issue, not a dead-code or broken-
commit issue.

**Recommendation.** Keep the plan's recommended resolution (plain-only at C3,
completed at C4) — it matches 001's first-consumer rule and avoids pulling the
whole Vault client into C3 for one variant. Two conditions: (1) C3's commit
message must state `git_url_for` is plain-only and vault-git lands at C4 (the
plan already says "flag the partial in C3's commit message" — make that
mandatory); (2) C3's acceptance test must use a plain/no-cred component repo so
the capability is provably exercised end-to-end without the missing variant. The
`git_url_for` enum/guard signature must be the **final** one at C3 (vault arm
returns an error or unreachable until C4) so C4 adds a match arm, not a
signature change — otherwise C3's public type churns at C4.

### S-3 — M2/C5a "assemble without push" is a test-only commit; acceptable, but call the trade-off precisely

**Problem.** M2/C5a (`ContainerBuilder.build`) assembles the image but cannot
push it (finish/push lands in C5b). Its only end-to-end exercise is `build()`
unit/integration tests; there is no pushed artifact a user can observe. The
git-commits smell test asks "what can a user/operator DO after this commit they
couldn't before?" — the honest answer for C5a is "nothing observable end-to-end;
the assembled image exists only inside the build container and is discarded."

**Impact.** Borderline against the capability-not-layer rule. It is **not** a
dead-code commit (every type/function added has a caller in `build()`, exercised
by tests), and `build()` is a real, testable unit. But it is closer to a "layer"
(assembly) than a "capability" (a pushed image) than the corpus's other commits.
The plan flags this itself and recommends keeping the split to isolate the
push-rc fix.

**Recommendation.** Acceptable to keep split — the push-rc fix (008, a real
Python bug) is genuinely cleaner isolated in C5b, and `build()` has true unit
coverage. But the C5a commit message must frame it as "assemble the container
image (verified by build() tests; push lands next)" so the history does not read
as a layer commit. If the milestone review prefers a strict capability boundary,
merge C5a+C5b into one C5 (sequential assemble+finish+push) — the plan offers
this and it is the safer default if the reviewer is unsure. Either is
defensible; the corpus should pick one before C5 implementation, not defer it.

### S-4 — M4/C8 Commit 1 is genuinely large; the precursor-split escape hatch is correctly gated, but the "leave python unused for one commit" window needs an explicit revert story

**Problem.** M4/C8 Commit 1 (the in-process cutover) bundles: descriptor
translation glue + the two-task panic-isolation model + cancellation rewrite (4
sites) + output-streaming source swap + supervisor field substitution + the
ContainerFile `COPY` of `cbsbuild` + deletion of `executor.rs`. The plan argues
this is atomic (the binary mount and the in-process call are interdependent; a
partial cutover is non-functional) and I agree — splitting the descriptor-
translation module out as a precursor would be dead code (no consumer until the
build task exists), which the plan correctly rules out. But this is the single
largest behavioral commit in the corpus and it leaves `python3`/wrapper present-
but-unused in the image for exactly one commit (Commit 2 removes them).

**Impact.** The commit is large but atomically coupled, so a >800-line commit is
justified here per the git-commits "when NOT to split" rule. The one-commit
dead-artifact window (python present but unused) is benign for `git bisect`
(Commit 1 compiles and works; the build runs in-process; python just sits
unused). The real risk is reviewability and revert blast radius: reverting
Commit 1 alone restores the subprocess path **only if** Commit 1 did not also
delete `executor.rs`/`output.rs` content that Commit 2 depends on — but here
Commit 1 deletes `executor.rs` entirely, so a revert of Commit 1 must restore
it.

**Recommendation.** Keep Commit 1 atomic (the coupling is real). Two conditions:
(1) the cutover and the deletion of `executor.rs`/`output.rs`-sentinel-parsing
must be in the **same** commit (Commit 1) — they are; confirm the plan does not
later try to defer the deletion, or the dead code lingers with `#[allow(...)]`;
(2) Commit 1's message must state it is the behavioral cutover and that
`python3`/wrapper are intentionally left in the image until Commit 2, so a
bisect/revert reader knows the window is deliberate. The plan's "highest-risk
wiring" note (register-before-attach, single completion-task owner) is correct
and matches 011 and the real `supervisor.rs`; preserve that verbatim.

### S-5 — M1 Commit 2 sizing is at-risk and the split escape hatch would create dead code; pre-commit a measurement gate

**Problem.** M1 Commit 2 bundles subprocess primitive + redaction + git wrapper
(2 ops) + `version_create_helper`/title/UUIDv7 + `get_image_desc` (fixed) + the
full clap CLI (globals, BOOL parser, `versions create` handler). The plan's note
concedes this "may approach/exceed ~800 lines" and says the subprocess+redaction
primitive "can be pulled into its own commit **only if** a consumer lands with
it (otherwise it is dead code)." That caveat is exactly right — the
subprocess/git primitive has no consumer until `version_create_helper` reads git
user/repo-root, so a primitive-only precursor commit would be dead code and fail
the smell test.

**Impact.** The escape hatch the plan names (split out the primitive) is the
**wrong** split — it manufactures the dead-code anti-pattern the corpus is built
to avoid. So if Commit 2 measures over budget, the plan's own suggested remedy
is invalid. The remaining valid options are: accept a justified

> 800-line commit (the git wrapper's first consumer genuinely is
> `version_create_helper`, so the coupling argument holds), or find a different,
> capability-clean seam (e.g. land `versions create` for a supplied `M.m.p`
> version first, add UUIDv7 + image-desc-skip second — but UUIDv7 is a settled
> M1 deliverable, so this fragments a single capability).

**Recommendation.** Keep Commit 2 together; if measured over ~800 lines, accept
it as a justified atomically-coupled commit (document the coupling in the commit
message), and **do not** split out the subprocess primitive (dead code). Update
the plan note to remove the misleading "pull the primitive into its own commit"
option, or qualify it as invalid-because-dead-code. This matches the corpus's
own first-consumer rule; the note currently half-contradicts it.

### S-6 — `descriptor_path` ownership: M5 defers it, but 006 places the helper in `cbscore-types` while M1 writes the path inline

**Problem.** 006 (Configurable location, M5) specifies a single pure helper
`descriptor_path(root, type, version) -> <root>/<type>/<version>.json` living in
`cbscore-types` as "the one source of truth for the path." M1/C1 writes the
descriptor to `<git-root>/_versions/<type>/<version>.json` using "the hardcoded
`_versions` default (configurable store is M5/C9 — `descriptor_path` helper
deferred)." So M1 hand-rolls the path-join inline, and M5 later introduces the
canonical helper — at which point M1's inline join must be refactored to call
it, or two path-construction sites diverge.

**Impact.** Minor but real: deferring `descriptor_path` to M5 means M1 has a
path-construction site that M5's "one source of truth" helper is supposed to
own. If M5 does not retro-fit M1's writer to use the helper, invariant 3
(on-disk layout parity) has two independent implementations. This is a latent
D2-duplication risk scheduled into the plan.

**Recommendation.** Either (a) land the trivial pure `descriptor_path` helper in
`cbscore-types` at M1/C1 (it is ~3 lines, has an immediate consumer — the M1
writer — so it is **not** dead code, and it pre-positions M5 to add only the
precedence ladder), or (b) explicitly note in M5/C9's scope that it must
refactor the M1 writer to call the new helper. Option (a) is cleaner and
cheaper: it makes M1 the helper's first consumer (consistent with the corpus's
rule) and removes the divergence risk entirely. The plan currently implies the
helper does not exist until M5, which forces option (b) by omission.

### S-7 — M3/C7's `s3_download_json` reuse assumes C4/C6 landed a JSON downloader; confirm the exact function name lands earlier

**Problem.** M3/C7 says it "reuses `Config`/`SecretsMgr`/`SecretsMgr.s3_creds`/
`s3_download_json` from C2/C4." 005 defines `s3_download_str_obj` (404 → None)
and the thin wrapper `s3_download_json`; the S3 read path
(`check_release_exists`, `check_released_components`) lands at **C6**, not C4.
M3's prose says "C2/C4" for the downloader reuse, but `list_releases` needs
`s3_download_json` over each listed key, and the only place a JSON object
download is specified to land is the C6 read path (005 tags
`s3_download_str_obj`/`check_release_exists` as C6). So M3/C7 depends on **C6**,
not C4, for the download primitive.

**Impact.** Low — M3 is sequenced after all of M2 (C2–C6) per the dependency
graph, so C6 has landed before C7 regardless; the capability ordering is safe.
The issue is purely a wrong cross-reference in M3's text ("from C2/C4" should be
"from C4/C6"), which could mislead an implementer into expecting the downloader
at C4. The README dependency graph and 001's design index (005 "first consumed
by C4 write, C6 object read, C7 listing") are correct; M3's prose is the
outlier.

**Recommendation.** Fix M3/C7's reuse line to read "reuses `Config`/`SecretsMgr`
from C2, `s3_creds` from C4, and `s3_download_str_obj`/`s3_download_json` from
**C6**." This aligns with 005's capability tags and the README graph. No
sequencing change needed.

---

## Minor observations 🟢

- **M0/C1 `resolver` note.** The plan correctly leaves `resolver = "2"` and
  notes bumping to 3 is out of scope. Verified the workspace is `resolver = "2"`
  and has no `[profile.*]`, so C0 adding `[profile.release] panic = "unwind"` is
  net-new (not a flip) — the plan's phrasing ("the workspace sets no `panic`
  today, so `unwind` already applies; pinning prevents a future flip") is
  accurate.

- **M4 `RunOpts` invariant-#4 amendment.** M4 correctly states the run name is
  `"cbs-" + trace_id.replace('-',"")[..12]` with `replace_if_exists = true`,
  matching `cbscore-wrapper.py:234` and 011. The amendment of
  `cbsd-rs/CLAUDE.md` invariant #4 (in-process arg, not `CBS_TRACE_ID` env) is
  faithful to 011. Make sure the actual `CLAUDE.md` edit lands in the same
  commit as the cutover so the invariant doc never lies about a removed env var.

- **M4 timeout default 7200 s.** Verified against `cbscore-wrapper.py:233`
  (`CBS_BUILD_TIMEOUT` default 7200) and 011. The plan's "never falls through to
  009's 4 h CLI default" is correct and well-targeted.

- **Fix-don't-reproduce placement spot-check.** All verified against the owning
  designs: partial-report-on-failure + temp-secret RAII + dead-secrets-write →
  M2/C2a (009/010); buildah push rc → M2/C5b (008); `can_sign` + `--force` →
  M2/C6 (007/008); `list_releases` arity → M3/C7 (005/006); `get_image_desc` →
  M1/C1 (006). Reproduced quirks F2 (silent component drop) and F3 (no-cwd) are
  scheduled at M2/C4 "with a clear error log," not fixed — matches ROADMAP
  and 007. Correct throughout.

- **M2/C2a config-type landing.** The keystone correctly lands the **config and
  secrets-file _format_** (not resolution) as the first config consumer (the
  runner marshals the secrets file), with `SecretsMgr`/Vault resolution deferred
  to C3/C4. This matches 004's "first consumer: C2 (config + secrets file), C4
  (SecretsMgr + Vault)" and 009's secrets-marshal step. Clean.

- **M1 config-independence.** Verified `cmd_versions_create` takes no `ctx`
  (Python `versions.py`), so "no config types land here" is correct; M1 needs
  only the component repo-URL-per-ref, resolved from component defs/overrides,
  not the full `load_components`/`CoreComponent` (007's). The plan is precise
  that `CoreComponent`/`load_components` is referenced-not-built at M1.

- **`get_image_desc` skip condition.** M1 correctly skips the image-desc note
  for a UUIDv7 **or patch-less** version (006: "a UUIDv7 or a patch-less `20.2`
  has nothing to key on and skips"). The Testable line covers both the resolve
  and the skip. Good.

- **Redaction structural test.** M1's testable (`format!("{:?}", cmd)` and the
  logged form contain `<CENSORED>`, plaintext reaches `exec`) maps invariant 4
  to a concrete test, and the plan flags verifying the trait has no
  `Debug`/`Display` supertrait — matches 003's compiler-enforced contract.

- **Correctness-invariant test coverage.** I checked each of 001's 10 invariants
  for an assigned test in a plan Testable line: 1 wire round-trip (M1 C1), 2 CLI
  parity (M1/M2 flag tables), 3 on-disk layout (M1 golden-file; see S-6 caveat),
  4 redaction (M1 C2), 5 report round-trip (M2 C2a), 6 failure isolation (M4
  panic gate), 7 binary portability (M0 C0 EL9 PID-1), 8 Vault auth order (M2
  C4), 9 S3 addressing (M2 C4 MinIO parity), 10 documented config (README,
  cross-cutting). All ten are assigned. The only soft spot is invariant 3, where
  S-6's `descriptor_path` divergence could split the layout source of truth.

- **README "14 commits" total.** Will need updating if S-1 (C4 split → 8 in M2)
  is adopted; track the count in `plans/README.md` and each milestone's progress
  table as the per-milestone breakdowns are approved (the README already says
  the exact boundaries are confirmed at each milestone's approval).

---

## Strengths

- **The capability-not-layer discipline is real, not decorative.** Every
  milestone leads with an operator/worker capability, and the plans explicitly
  schedule foundational code (subprocess primitive, git/podman/buildah/skopeo
  wrappers, S3 client, Vault client) into its first-consumer commit with the
  "lands here" evidence. The corpus actively refuses the dead-code precursor
  split even when it would make a commit smaller (M1 Commit 2 note, M4 Commit 1
  note) — that is the anti-pattern the original 000 review flagged, and the
  plans internalize it correctly.

- **Self-aware sizing notes.** Each plan ends with a "Notes for the plan-review"
  block that flags its own at-risk seams (C4 oversize, the vault-git seam,
  C5a/C5b, M4 Commit 1 atomicity). This is exactly the right altitude for a
  plan: it surfaces the decisions a reviewer must make rather than hiding them.
  Most of my significant concerns are confirmations-with-conditions of risks the
  plans already named.

- **High design fidelity on load-bearing details.** I verified the named
  contracts against the real code and designs: the four `exec.kill()` sites and
  their line numbers, the register-then-attach order,
  `executor: Option<BuildExecutor>`/`output_task` substitution,
  `MAX_REPORT_SIZE = 65_536`,
  `BuildFinishedStatus { Success, Failure, Revoked }`, the `list_releases` arity
  bug at `cmds/versions.py:129`, the parse parametric cases at
  `utils.py:160-241`, the `_validate_version` minor+patch requirement, the
  wrapper's config-load-first/translate/run sequence and 7200 s default. Every
  one matched. The plans did not invent a contract or mis-assign an ownership
  that I could find.

- **The fix-don't-reproduce vs reproduce-the-quirk split is placed correctly**
  in every case (see the spot-check above), and the reproduced quirks are
  explicitly "reproduced with logs," not silently changed — faithful to the
  port's stated philosophy and to ROADMAP.

- **M4's panic-isolation wiring is described precisely** — single completion-
  task owner, register-before-spawn, first-terminal-wins arbitration, the benign
  revoke/completion race, the teardown-barrier await on shutdown. This is the
  subtlest, highest-risk part of the whole port, and the plan mirrors 011's v4
  design and the real supervisor structure faithfully.

---

## Open questions

1. **C-1:** Will C0 add a PR/push-triggered CI workflow for the Rust workspace,
   or is the musl proof being treated as a local implementer gate? If the
   latter, the plan/design wording ("proven in CI before subsystem work") must
   change to match reality.

2. **C-2:** What is the intended worker-image topology after the cutover — a new
   python-free `worker-base-rs`, or re-pointing `cbsd-rs-worker` at
   `alpine:3.21` directly? And does the **dev** worker keep python (for
   cargo-watch) or also move? The plan must commit to one before M4 Commit 2.

3. **S-1:** Is the C4a/C4b split being promoted into the committed breakdown (8
   commits in M2), or held as an at-implementation decision? It should be
   committed now given the certainty of the oversize.

4. **S-3:** For C5, does the milestone owner keep the C5a/C5b split (isolating
   the push-rc fix, accepting a test-only C5a) or merge into one C5? The corpus
   should pick before C5 implementation.

5. **S-6:** Does `descriptor_path` land in `cbscore-types` at M1 (with the M1
   writer as its first consumer), or does M5 own both the helper and the
   refactor of M1's inline path-join? Picking M1 removes the duplication risk
   against invariant 3.

6. The README's "14 commits" total — will it be kept in sync as the C4 (and
   possibly C5/M4) boundaries are finalized at each milestone's approval gate?

---

## Confidence-scoring

Scoring the plan corpus as a plan artifact (readiness to drive implementation
faithfully without re-work). Each distinct finding is a separate deduction.

| Item                                                                                                           | Points | Description                                                                                              |
| -------------------------------------------------------------------------------------------------------------- | -----: | -------------------------------------------------------------------------------------------------------- |
| Starting score                                                                                                 |    100 |                                                                                                          |
| C-1: musl de-risk proof wired to release-only CI (defeats "before subsystem work")                             |    -15 | D1/D12-class — the C0 capability (a pre-work gate) is not actually delivered by the planned CI placement |
| C-2: M4/C8 Commit 2 "drop python3" under-scopes the shared python `worker-base` stage                          |    -10 | D1-class — the cleanup as written is non-functional or unplanned-size against the real ContainerFile     |
| S-1: M2/C4 acknowledged-oversize, split left as a maybe not the committed plan                                 |     -8 | D12-class — a single C4 fails the smell test (>800 lines, two new deps, two stages)                      |
| S-2: C3 vault-git seam leaves `git_url_for` partial; functional-only-for-plain not mandated in commit msg/test |     -4 | D8-class — contract clarity; capability still deliverable                                                |
| S-3: M2/C5a is a test-only "assemble without push" commit                                                      |     -4 | D12-class — borderline layer-vs-capability; not dead code                                                |
| S-4: M4/C8 Commit 1 large; revert/window story needs to be explicit                                            |     -3 | D12-class — coupling justifies size; revert semantics need a commit-message note                         |
| S-5: M1 Commit 2 split escape-hatch would create dead code (invalid remedy)                                    |     -4 | D12-class — the named remedy contradicts the no-dead-code rule                                           |
| S-6: `descriptor_path` deferred to M5 risks a second path-construction site                                    |     -4 | D2-class — latent duplication against invariant 3                                                        |
| S-7: M3/C7 cross-ref says downloader reuse "from C2/C4"; it lands at C6                                        |     -3 | D10-class — broken cross-reference vs 005's capability tags                                              |
| **Total**                                                                                                      | **45** |                                                                                                          |

Arithmetic: 100 − (15 + 10 + 8 + 4 + 4 + 3 + 4 + 4 + 3) = 100 − 55 = **45**.

The mechanical floor understates the corpus quality: the two Critical items are
infrastructure-reality corrections (one CI file, one ContainerFile scope note),
not design defects, and the seven significant/minor items are mostly
confirmations of risks the plans themselves raised with sound recommended
resolutions. The deductions are scored against an ideal "ready to implement with
zero plan edits" bar; the corpus is closer to "ready after the C0 CI gating and
M4 image-cleanup scope are corrected, with the C4 split promoted." I record the
table score at **45** per the rubric's mechanical application, and note the
**adjusted reviewer judgment is ~72** once the infra-reality nature of C-1/C-2
and the self-flagged-and-resolved nature of S-1/S-3/S-4/S-5 are weighted — i.e.
"significant issues, address C-1/C-2 and promote the C4 split before
proceeding," not "major rework."

Per the rubric the mechanical 45 lands in "major rework needed" only because the
two Criticals each gate a milestone's first commit; in substance the corpus is a
sound plan with two infra corrections and a set of sizing decisions to finalize.

---

## Verdict

**Approve with conditions.**

The corpus is design-faithful, capability-sliced, and honestly self-critical.
Proceed milestone by milestone, but the following must be resolved before the
gated milestones begin:

- **Before M0 lands:** fix C-1 — give the musl de-risk proof a PR/push-triggered
  CI gate (new workflow) so it actually runs before subsystem work, or restate
  the "proven in CI before subsystem work" promise to match a local gate.
- **Before M4 Commit 2 lands:** fix C-2 — rewrite the python-removal step
  against the real ContainerFile topology (shared `python:3.13-alpine`
  `worker-base` stage feeding prod **and** dev workers).
- **Before M2 C4 is implemented:** adopt S-1 — promote the C4a/C4b split into
  the committed breakdown (update the M2 progress table and README total).
- **Address during milestone approval:** S-2 (mandate the C3 plain-only commit
  message + plain-repo acceptance test), S-3 (decide C5 split vs merge), S-5
  (remove the invalid dead-code split option from M1's note), S-6 (land
  `descriptor_path` at M1 or task M5 with the refactor), S-7 (fix the C6 cross-
  reference).

None of these is a design defect or a fundamental flaw; the underlying designs
are GO and the plans track them faithfully. The conditions are infrastructure-
reality corrections and sizing finalizations, after which the corpus is ready to
implement.
