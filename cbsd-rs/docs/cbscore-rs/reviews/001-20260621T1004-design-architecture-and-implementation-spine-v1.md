# Adversarial design review — architecture & implementation spine (001)

- **Type:** design review (adversarial, design-level)
- **Reviews:** design seq 001,
  `design/001-20260621T0949-architecture-and-implementation-spine.md`
- **Date:** 2026-06-21
- **Verdict:** GO WITH CHANGES. The spine genuinely clears the prior review's
  blockers (B1, B2 intent, H1, H3, H4, H5, M2, M3), but it carries a small set
  of precision and internal-consistency defects — two of which name Rust
  mechanisms (`catch_unwind`, task-drop → `podman stop`) that do not work as
  described for in-process async orchestration. None blocks the architecture;
  all should be folded in before the per-milestone plans (`001-NN`) are written,
  because the plans derive their commit boundaries and subsystem interfaces from
  this document.
- **Confidence:** 78 / 100 (see table).

## What is being reviewed

Design 001 is the orientation document for the Rust port of the Python `cbscore`
build library. It fixes the three-crate layout (`cbscore-types` ← `cbscore` ←
`cbsbuild`, plus `cbsd-worker` ← `cbscore`), the cross-cutting conventions
(errors, logging, schema versioning), the musl build target, the
failure-isolation model, and — the load-bearing artifact — a **capability commit
map** (C0–C11) that sequences the work and claims to be free of layer-by-layer
and dead-code commits. It asserts it resolves the findings of the prior review
(000): B1, B2, H1, H3, H4, H5, M2 (and folds H2 into the C4 CLI line).

## Method

Every claim was checked against the Python `cbscore` source
(`cbscore/src/cbscore/`), which is the behavior being ported and therefore the
specification, and against the cbsd-rs integration context
(`cbsd-worker/src/build/`, the workspace `Cargo.toml`, and
`container/ContainerFile.cbsd-rs`). The capability commit map was assessed with
the `git-commits` smell test: for each commit, does it deliver an
operator/worker capability, and does the "foundational code landing here" column
have a real first consumer in that same commit, or is it dead code? Rust-runtime
claims (`catch_unwind`, task-drop cancellation, `panic = "unwind"`) were checked
against how the primitives actually behave, not taken on assertion.

## Confidence score

Starting from 100, each distinct defect deducts by severity. The criteria from
`confidence-scoring` (D1–D12) are code-review triggers; for a design document
they are mapped onto severity bands, mirroring review 000's adaptation for
consistency within this doc set.

| Deduction      | Pts    | Finding                                                             |
| -------------- | ------ | ------------------------------------------------------------------- |
| Starting score | 100    |                                                                     |
| High H1        | −7     | `catch_unwind` is the wrong primitive for in-process async (B2)     |
| High H2        | −6     | "task-drop → `podman stop`" is not a runnable mechanism (B2)        |
| Med M1         | −4     | Subsystem index vs commit map: Vault client dead in C1 (004→C1)     |
| Med M2         | −3     | C2 omits `get_image_desc` as a real consumer (008 maps only C7/C8)  |
| Med M3         | −3     | C3 keystone (`versions list`) has a broken Python reference         |
| Med M4         | −2     | C1 "non-interactive" has no Python parity reference                 |
| Low L1         | −2     | C2 "git-SHA resolution is the first consumer" is factually wrong    |
| Low L2         | −2     | `panic = "abort"` "musl flips for size" mischaracterizes Cargo      |
| Low L3         | −1     | musl build asserts an explicit `--target` the existing build omits  |
| Note N1        | 0      | `cbscommon-rs` lift-out target collides with existing `cbsd-common` |
| **Total**      | **78** | Fold the changes into the corpus before plans are written           |

## Findings

### High H1 — `catch_unwind` is the wrong primitive for in-process async failure isolation (B2)

**Design claim.** "The worker wraps the build task in a `catch_unwind` boundary
that maps a caught panic onto the same build-failure path the subprocess exit
code produces today" (001, §"Failure isolation"), and correctness invariant 6
restates it: "in-process build wrapped in `catch_unwind`."

**What the runtime actually shows.** `std::panic::catch_unwind` wraps a
synchronous closure (`FnOnce() -> R`). It does **not** catch a panic raised
across an `.await` point, which is where essentially all of the host-side
orchestration lives — `podman_run` (`runner.py:288`), `prepare_components`, the
S3 uploads, all `async`. To catch a panic in an async task you either (a) read
it off the tokio task boundary — `JoinHandle::await` yields `Err(JoinError)` and
`JoinError::is_panic()` distinguishes a panic (this is the mechanism review 000
itself pointed at), or (b) use `futures::FutureExt::catch_unwind`, which
requires the wrapped future to be `UnwindSafe` — a bound much orchestration code
will not satisfy without `AssertUnwindSafe` and careful reasoning about poisoned
state.

**The gap.** The named primitive cannot wrap the thing the design says it wraps.
An implementer following the spine literally would write
`std::panic::catch_unwind(|| build_future)` — which compiles, catches nothing
useful, and silently defeats the isolation invariant. The intent (map a panic in
in-process orchestration onto the build-failure path) is correct and achievable;
the mechanism is misnamed.

**Recommended change.** State the mechanism as the tokio task boundary: the
build runs in a spawned task (or `JoinSet`), and the worker maps
`JoinError::is_panic()` onto the same `BuildFinishedStatus::Failure` path the
subprocess exit-code classification produces today (`executor.rs:275`,
`classify_exit_code`). If a synchronous `catch_unwind` is wanted anywhere, scope
it to a specific synchronous section and say so. Keep the detail in 009/011 but
fix the primitive here, since invariant 6 is asserted as fixed.

### High H2 — "build cancellation is task-drop → runner `podman stop`" is not a runnable mechanism (B2)

**Design claim.** "Build cancellation is task-drop / cancel → the runner issues
`podman stop` on the builder container … This replaces today's
kill-the-process-group" (001, §"Failure isolation"), restated in the B2
resolution.

**What the runtime actually shows.** Rust has no async `Drop`. When a future is
dropped (task cancelled), its destructors run synchronously; they cannot
`.await`. `podman stop` is an async subprocess call (`podman_stop`,
`runner.py:343-345`). Therefore "task-drop → runner issues `podman stop`" cannot
happen on the drop path as written — the drop returns before any `podman stop`
could be awaited, and the builder container is leaked. Today's worker does not
rely on this: it sends SIGTERM/SIGKILL to the subprocess **process group** from
a separate synchronous call (`executor.rs:215-251`, `libc::kill(pgid, …)`),
which is exactly why the subprocess model contains the build. The in-process
port loses that process-group hook and must replace it with an explicit
cancellation signal.

**The gap.** The cancellation story is stated as a property ("is task-drop →
`podman stop`") rather than a mechanism, and the stated property is not
achievable by dropping a future. The correct shape is a
`tokio_util::sync::CancellationToken` (or a select-on-shutdown) that the runner
observes and, **before** returning, awaits `podman stop` on the named container
— the runner already knows the container name (`ctr_name`, `runner.py:282`) and
already issues `podman stop` on timeout, so the call site exists; only the
trigger is new.

**Recommended change.** Replace "task-drop → `podman stop`" with an explicit
cancellation token the runner selects on, awaiting `podman stop <ctr_name>`
before unwinding. Note that the container name must be plumbed to the
cancellation handler so the stop targets the right container. Defer the wiring
to 009/011 but correct the asserted mechanism here.

### Medium M1 — subsystem index vs commit map disagree: the Vault client is dead code in C1

**Design claim.** The subsystem index maps "004 — Configuration, secrets &
Vault" to "Consumed by **C1**." C1's capability is `config init` / `init-vault`
producing "config + secrets + vault files non-interactively," and its "lands
here" column lists "Vault config."

**What the code shows.** `config_init_vault` (`config.py:40-105`) only
**writes** a `vault.yaml`: it constructs a `VaultConfig` and calls `.store()`.
It never instantiates a `Vault` backend and never calls
`check_vault_connection()`. The Vault **client** (the thing that would pull in
`vaultrs`) has no caller until C3: `versions list` constructs
`SecretsMgr(config.get_secrets(), vault_config=…)`, whose `__init__` calls
`check_vault_connection()` when a vault is configured (`mgr.py:60-66`), and
`s3_creds` → `storage_get_s3_creds` → `vault.read_secret` for a
`StorageVaultS3Secret` (`storage.py:48-56`, `vault.py:56-75`).

**The gap.** If the implementer follows the index (004 → C1) and lands the Vault
client crate in C1, it is dead code until C3 — precisely the "foundational code
as a standalone layer" anti-pattern the spine says it avoids. The commit-map
column ("Vault config", i.e. types + load/store) is narrower and correct; the
index is wrong.

**Recommended change.** Correct the index: 004's Vault **config** (types, YAML
load/store) is consumed by C1; the Vault **client** (`vaultrs`, login,
`read_secret`, `ces-kv`) is first consumed by C3 and must land there, not in C1.
Split the 004 row or annotate it "C1 (config), C3 (client)."

### Medium M2 — C2 omits `get_image_desc` as a real first consumer; conflicts with the 008→C7/C8 mapping

**Design claim.** C2 (`versions create`) "lands here": "version types, parse
helpers (`get_version_type`), subprocess primitive + redaction + git wrapper."
The subsystem index maps "008 — Containers & images" to "Consumed by C7, C8."

**What the code shows.** `version_create` ends by calling
`get_image_desc(desc.version)` (`versions.py:111`). `get_image_desc`
(`images/desc.py:42-91`) walks a `desc/` directory under the git repo root,
parses `ImageDescriptor` JSON, and resolves a version → image descriptor — i.e.
it pulls in image-descriptor types and a filesystem walk. It is invoked by C2's
command, not by C7/C8.

**The gap.** Either C2 must also land `get_image_desc` + the `ImageDescriptor`
type (making C2 larger and contradicting the index's claim that image code is
first consumed at C7/C8), or the port intends to drop/defer that trailing "image
descriptor missing?" check. The spine is silent, so a downstream implementer
cannot tell which. If C2 ships the git wrapper and `versions create` but
silently omits `get_image_desc`, that is a behavioral parity gap the spine never
flags.

**Recommended change.** Decide explicitly: either (a) C2 lands a minimal
`get_image_desc` + `ImageDescriptor` and the index records "008 consumed by C2,
C7, C8," or (b) the trailing image-descriptor check is a documented intentional
drop for C2 (and deferred), in which case say so in the C2 row and in the
CLI-parity notes.

### Medium M3 — the C3 keystone (`versions list`) has a broken Python reference

**Design claim.** C3 delivers "`cbsbuild versions list --from <addr>` lists
releases" and lands "S3 client read path + release-read types." The goals
section promises "faithful behavioral parity with Python `cbscore`."

**What the code shows.** The Python `versions list` path is non-functional in
the current source. `versions_list` calls
`list_releases(secrets, s3_address_url)` with two arguments (`versions.py:129`),
but `list_releases` requires four — `(secrets, url, bucket, bucket_loc)`
(`releases/s3.py:237-239`). Invoking the command would raise
`TypeError: list_releases() missing 2 required positional arguments: 'bucket' and 'bucket_loc'`.
The bucket and location are not sourced anywhere in the command path; `--from`
supplies only the URL.

**The gap.** The spine pins C3's capability — and the landing of the entire S3
read path — to a command whose reference behavior does not run. "Faithful
behavioral parity" has no working behavior to port: the porter must reconstruct
intent (do `bucket`/`loc` come from `config.storage.s3.releases`? from the
`--from` address?), which the spine leaves unspecified. (The read path itself is
otherwise correctly placed: `list_releases` uses both `s3_list` and
`s3_download_str_obj` — `releases/s3.py:242,259` — so C3 genuinely exercises
both list and download, justifying the whole read path landing there.)

**Recommended change.** Note in the C3 row that the Python reference is broken
and specify the intended C3 behavior: where `bucket`/`loc` come from, and what
`--from` denotes (host only, or host + bucket + loc). This is the kind of source
defect the port should fix, not faithfully reproduce.

### Medium M4 — C1's "non-interactive" config init has no Python parity reference

**Design claim.** C1 delivers "`config init` / `init-vault` produce config +
secrets + vault files **non-interactively**," with interactive UX deferred to
C11.

**What the code shows.** The Python `config init` and `init-vault` are
**interactive only** — built entirely on `click.confirm` and `click.prompt`
(`config.py:45-105`, `config.py:119-159`, and onward). There is no
non-interactive mode in the source.

**The gap.** C1 is therefore not a port of existing behavior; it is a new
non-interactive mode invented for the port (presumably driven by flags). That is
a reasonable design choice, but the spine presents C1 under the parity framing
without noting that the behavior it ports does not exist in Python. A downstream
implementer has no reference for the flag set, defaults, or file layout C1 must
produce.

**Recommended change.** State that C1's non-interactive mode is new (not a
parity port), and either specify its flag/file contract here or point to where
004/010 specify it. Confirm that the on-disk output (the config, secrets, and
vault YAML files) matches what the interactive Python path writes, so the two
modes converge.

### Low L1 — C2's stated git-wrapper consumer ("git-SHA resolution") is factually wrong

**Design claim.** C2 lands "subprocess primitive + redaction + git wrapper
(git-SHA resolution is the first consumer)."

**What the code shows.** The `versions create` path never resolves a git SHA.
Its git calls are `get_git_user` (`git config user.name` / `user.email`,
`git.py:73-87`) and `get_git_repo_root` (`git rev-parse --show-toplevel`,
`git.py:90-97`); `get_image_desc` also calls `get_git_repo_root`. `git_get_sha1`
(`git.py:379-387`, `rev-parse HEAD`) is not reachable from the create path
(verified: no SHA-resolution call site in `versions/` or `core/component.py`).

**The gap.** The commit boundary is sound — the git wrapper IS legitimately
first-consumed in C2 — but the stated justification names the wrong operation,
which signals the consumer was asserted rather than verified and could mislead
an implementer into landing the wrong git surface first.

**Recommended change.** Replace "git-SHA resolution is the first consumer" with
"git user/email (`git config`) and repo-root (`rev-parse --show-toplevel`)
resolution is the first consumer."

### Low L2 — the `panic = "abort"` justification mischaracterizes Cargo

**Design claim.** "The worker release profile must remain `panic = "unwind"`
(musl release builds sometimes flip to `abort` for size — that is prohibited
here)."

**What the code shows.** Cargo never auto-flips `panic` to `abort` for musl or
for size; the default is `unwind` and stays `unwind` unless a profile explicitly
sets `panic = "abort"`. The workspace `Cargo.toml` sets no `panic` value
anywhere (verified — no `[profile.release]` `panic` key in the workspace or
worker manifests), so the default `unwind` already applies.

**The gap.** The conclusion (keep `unwind`, and pin it explicitly so a later
edit cannot silently break isolation) is correct and worth doing. The stated
reason ("musl flips for size") is not a real Cargo behavior and weakens the
invariant's credibility.

**Recommended change.** Reword: "the worker release profile must pin
`panic = "unwind"` explicitly so no future profile edit can switch it to `abort`
and silently void the failure-isolation invariant." Drop the "musl flips for
size" rationale.

### Low L3 — the musl target asserts an explicit `--target` the existing build does not use

**Design claim.** "`cbsbuild` is built `x86_64-unknown-linux-musl` … This
matches the existing Alpine/musl `rust-builder` stage in
`container/ContainerFile.cbsd-rs`."

**What the code shows.** The `rust-builder` stage is `FROM alpine:3.21` and runs
`cargo build --release --workspace` with **no** `--target` flag
(`ContainerFile.cbsd-rs:91,118`). On Alpine, rustup's default host triple is
already `x86_64-alpine-linux-musl`, so binaries are static musl implicitly — not
via an explicit `x86_64-unknown-linux-musl` target. Separately, the worker image
COPYs only `cbsd-worker` (`ContainerFile.cbsd-rs:158`); there is no `cbsbuild`
artifact built or copied today.

**The gap.** The match is real in effect (Alpine ⇒ musl) but the design
overstates it as an explicit target triple. More importantly, two concrete
Containerfile changes the spine depends on are not yet present and are only
implied: (1) `cbsbuild` must be a build output (the workspace must include the
crate so `cargo build --workspace` produces it), and (2) the worker image must
`COPY` the `cbsbuild` artifact to `/usr/local/bin/cbsbuild`. The B1 resolution
names the COPY; it does not name the "add `cbsbuild` to the workspace build"
step.

**Recommended change.** State that the existing stage yields musl implicitly via
the Alpine host (no `--target` needed unless building off-Alpine), and enumerate
both required Containerfile changes (workspace membership so the binary is
built; explicit COPY into the worker image) as part of C0/C9.

### Note N1 — the `cbscommon-rs` lift-out target collides with the existing `cbsd-common`

The lift-out invariants name a "future `cbscommon-rs` sister crate" for
`utils::git` and `utils::subprocess`. The workspace already contains a member
named `cbsd-common` (`cbsd-rs/Cargo.toml` members list). Two near-identical
names (`cbsd-common` vs `cbscommon-rs`) in one workspace invite confusion. Pick
one home for shared primitives and name it unambiguously, or state explicitly
why they are distinct.

## What the spine gets right (verified)

These prior-review resolutions check out against the source and should not be
re-litigated:

- **H1 (report round-trip).** Invariant 5 matches the Python exactly: the report
  is written in-container to the scratch mount (`builder.py:247-251`, including
  the skip path at `builder.py:134`), read on the host **before** the rc check
  (`runner.py:320-333`), and unlinked after read (`runner.py:333`), with the rc
  raise afterward (`runner.py:335`). Partial-on-failure is preserved.
- **H3 (`CBS_DEBUG`).** Forwarding `-e CBS_DEBUG=<1|0>` from the host's
  effective debug state matches `runner.py:290-294`.
- **H4 (S3).** Credentials from the secrets store injected as a static provider,
  `endpoint_url` from the secrets hostname, path-style to be validated — matches
  `s3.py:84,92-100` and `storage.py:25-56`. The hedge on path-style is
  appropriate: the Python sets no explicit addressing style.
- **H5 (Vault).** AppRole → userpass → token is the real order
  (`vault.py:165-184`); `ces-kv` is the hardcoded KV v2 mount (`vault.py:61`).
  Both recorded correctly.
- **M2 (schema policy).** The pragmatic additive-no-bump / breaking-bump policy
  is internally consistent and replaces the prior self-contradiction.
- **M3 (`get_version_type`).** Correctly described as a name→type lookup over
  the four labels, distinct from `parse_version` (`versions/utils.py:127-132`).
- **Commit-map consumers (mostly).** C4 (`podman_run`/`podman_stop`,
  `runner.py:288,345`), C7 (buildah wrapper, `utils/buildah.py`), and C8
  (`skopeo_image_exists`, `images/skopeo.py:159`; `check_released_components`,
  `releases/s3.py:163`) all have real first consumers in their stated commits.

## C0 assessment (git-commits skill)

C0 ("a static `cbsbuild` runs as PID 1 in rockylinux:9, prints version") fails
the skill's core test — "what can an operator DO after this commit?" — by the
design's own admission ("C0 delivers nothing an operator uses"). It also pulls
`aws-sdk-s3` and `vaultrs` into the tree solely to prove static linkage, with no
real consumer until C3 (S3 read, Vault client) and C1/C3 respectively. That is
the infra-merges-into-next / dead-dependency shape the skill says to fold
forward.

The author pre-empted this by declaring C0 a deliberate one-time de-risk gate.
That is defensible **only** if the B1 risk (musl + the crypto/TLS transitive
stack) is real enough to warrant a spike, which review 000 established it is.
The acceptable resolutions are: (a) keep C0 as an explicit throwaway de-risk
spike whose scaffolding (workspace members, empty crate skeletons) folds into
C1, with the musl-linkage proof living in CI / the acceptance gate rather than
as a shipped dependency edge; or (b) merge C0's scaffolding into C1 and run the
musl-linkage validation as a CI job, not a commit. Either is fine; the spine
should pick one and not present C0 as both "delivers nothing" and "its own
commit" without justifying why the dead dependency edge is worth carrying from
C0 to C3. As written it is a borderline pass — call it explicitly rather than
leaving the contradiction in the prose.

C4 and C7 are flagged by the design as oversize and deferred-to-split at plan
time. That is the right call; the only requirement is that the split be
**capability-based, not layer-based** (the design's parenthetical "C4: container
mechanism → +`prepare_builder`" and "C7: descriptor/repos parse → buildah
assembly" are capability seams, so the intent is sound). Verify at plan time; no
deduction now.

## Verdict

**GO WITH CHANGES.** The architecture (three-crate split, dependency direction,
lift-out invariants), the build-target resolution, the errors/logging/schema
conventions, and the capability-first framing of the commit map are sound, and
the spine has correctly cleared the prior review's blockers H1/H3/H4/H5/M2/M3
and the intent of B1/B2. Before the per-milestone plans (`001-NN`) are written,
fold in:

1. **B2 mechanisms (H1, H2 here).** Replace `catch_unwind` with the tokio task /
   `JoinError::is_panic()` boundary; replace "task-drop → `podman stop`" with an
   explicit `CancellationToken` the runner awaits a `podman stop <ctr_name>` on.
   Fix both in §"Failure isolation" and invariant 6, since they are asserted as
   resolved.
2. **Index/map consistency (M1, M2).** Vault client lands in C3, not C1; decide
   and record whether `get_image_desc` lands in C2 or is a documented C2 drop.
3. **C3 / C1 reference honesty (M3, M4).** Note that the Python `versions list`
   is broken and specify intended C3 behavior; note that C1's non-interactive
   mode is new (not a parity port) and specify its contract.
4. **Precision fixes (L1, L2, L3, N1).** Correct the C2 git-consumer wording,
   the `panic` rationale, the musl-target/Containerfile claims, and the
   `cbscommon-rs`/`cbsd-common` naming.

None of these is structural; all are corrections to assertions that an
implementer would otherwise take literally. With them folded in, the spine is
ready to drive the milestone plans.
