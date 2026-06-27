# 001 — Implementation Review v2: M2 Build Keystone (C2a/C2b/F2)

Scope: commits `816a82a..ceaff98` on branch `wip/cbscore-rs`.

Five commits reviewed:

- `25f1d73` — C2a-1: config and secrets file formats
- `57d043e` — C2a-2: container-spawn and build-report primitives
- `b3cce70` — C2a-3: host runner + in-container Builder skeleton + CLI
- `8eb40bf` — C2b: `prepare_builder` + `out_cb` streaming
- `ceaff98` — F2: strict `CoreComponent` validation in `versions create`

Authoritative designs consulted: 001 (spine/invariants), 003
(subprocess/redaction), 004 (config/secrets), 007 (builder pipeline), 009
(runner/two-phase), 010 (CLI surface). Python source of truth:
`cbscore/runner.py`, `cbscore/utils/__init__.py`, `cbscore/utils/podman.py`,
`cbscore/builder/prepare.py`, `cbscore/utils/secrets/models.py`.

---

## Executive Summary

The M2 keystone is a disciplined, well-structured body of work. The core
invariants — report-before-rc-check (invariant 5), staging-dir RAII cleanup
(secrets never outlive the build), select!-on-token cancellation with explicit
`podman_stop(name)`, and the F2 strict-component fix — are all implemented
correctly and tested. Design fidelity is high: argv order, mount table, env,
device flags, and the in-container runner-build argv all match their Python
counterparts and the owning designs.

There are no correctness blockers. The most important finding is a **spec/design
deviation in `out_cb` behavior**: the implementation collects-AND-streams,
whereas design 003 specifies collect-OR-stream (empty strings when a callback is
set). This is a deliberate divergence that the implementation documents clearly,
but the design has not been updated to reflect it, which creates a contract
mismatch every future reader will hit. Secondary concerns are the `Debug` derive
on the four live secret-value enums (a latent leak path if a `{:?}` lands in a
log at a future callsite), the `unsafe set_var` in `runner_build` requiring a
re-verification of the safety argument as Tokio's thread model evolves, and a
minor but real ambiguity in the commit split for C2a.

Recommendation: **approve with conditions** — address the design-003 deviation
and the `Debug`-on-secrets issue before the next commit.

---

## Critical Issues

### C1 — `Debug` on live secret-value types (invariant 4 risk)

**Problem.** `GitSecret`, `StorageSecret`, `SigningSecret`, and `RegistrySecret`
in `cbscore-types/src/secrets.rs` all `#[derive(Debug)]`. These enums carry
plaintext values directly in their fields (`ssh_key`, `password`, `private_key`,
`passphrase`, etc.). Design 003's redaction contract (`SecureArg` deliberately
has no `Debug` supertrait) is the right approach for _command arguments_, but
the secret-value enums that represent the _content_ of a secrets file are
unprotected: any code path that does `tracing::debug!("{:?}", my_secret)`, logs
a `RunnerError`, or `format!`s an enum containing one of these for any reason
will dump plaintext keys and passwords.

**Impact.** Today the runner never formats the `Secrets` struct directly. But
the type is `pub`, will be consumed by C3 (`SecretsMgr`), C4a (Vault
resolution), and C5b (registry creds), and future callers will `{:?}`-format
error context naturally. The redaction contract (invariant 4) says a secret
"cannot be `{:?}`/`{}`-formatted directly"; the enums violate this structurally.

**Recommendation.** Remove `#[derive(Debug)]` from `GitSecret`, `StorageSecret`,
`SigningSecret`, and `RegistrySecret`. Implement `fmt::Debug` manually for each
— either as a summary without values
(`"GitSecret::PlainSsh { username: \"u\", ssh_key: <REDACTED> }"`) or using the
`sensitive` pattern from design 003's `CmdArg`. The raw-wire helper structs
(`GitSecretRaw`, etc.) do not need this treatment since they have no `pub`
exposure, but confirm they also carry no `Debug` derive. This is a hard
requirement before C3 adds callers that will naturally format error context.

---

## Significant Concerns

### S1 — Design 003 `out_cb` contract divergence is undocumented in the design

**Problem.** Design 003 states: "If `out_cb` is set, each line is awaited
through it and `stdout`/`stderr` in the result are left empty." The
implementation in `subprocess.rs` deliberately diverges: it streams each line
through `out_cb` AND collects it into the output strings. The commit message and
the module doc string document this divergence clearly, and the reasoning is
sound (a failed step retains its stderr). However, **design 003 has not been
updated** to reflect the decided behavior. Any reviewer reading design 003
against the code sees a contradiction with no documented resolution.

**Impact.** The next engineer reading design 003 will report this as a bug. More
concretely: the C2b test `out_cb_streams_each_line_and_still_collects` pins the
_actual_ behavior, but the design remains the _old_ behavior. If someone writes
a future consumer that relies on empty strings when `out_cb` is set (trusting
the design), they'll get a surprise.

**Recommendation.** Amend design 003 to reflect the decided behavior: "If
`out_cb` is set, each line is streamed through it as it arrives AND still
collected into the result strings, so a failed step retains its stderr." This is
a one-sentence change; it does not require a new design version — it corrects an
inaccuracy in the existing design. The module-level comment in `subprocess.rs`
already says the right thing; the design must match.

### S2 — `unsafe std::env::set_var` safety argument is fragile for tokio

**Problem.** `build.rs:apply_home_hook` uses
`unsafe { std::env::set_var("HOME", "/runner") }` with the safety justification:
"this is the first action of the in-container `runner build` entry, before any
concurrent environment access in this short-lived PID-1 process; no other thread
reads HOME at this point." The assertion "no other thread reads HOME at this
point" relies on `set_var` being called before `#[tokio::main]` spins up Tokio's
thread pool.

Looking at `main.rs`, the invocation chain is: `main()` (tokio main) → match
`Command::Runner` → `cmds::build::runner_build()`. This means `apply_home_hook`
is called **inside the tokio async runtime**, after the thread pool is running.
The standard library's `set_var` is UB if any other thread reads the environment
concurrently; Tokio worker threads may be doing so at that instant (e.g., via
`std::env::var` calls in library initialization, thread-local setup, or future
background work). The current comment says "before any concurrent environment
access" but this is not verifiable once we're inside a `#[tokio::main]` context.

**Impact.** This is technically UB. In practice it is unlikely to cause a
problem in this specific single-purpose PID-1 binary, but the safety comment is
incorrect, and it sets a precedent. If a future commit adds library
initialization that touches the environment (e.g., TLS provider detection,
signal handler setup), the race becomes real.

**Recommendation.** Either: (a) move `apply_home_hook` before the `tokio::main`
entry point by implementing it as a standalone `fn main()` that calls `set_var`
and then re-enters via `tokio::main`, which is the correct pattern; or (b)
restructure so the `runner build` dispatch is in a plain `fn main()` shim that
applies the hook before starting Tokio. Option (a) is the idiom. This requires
restructuring `main.rs` to detect `runner build` before spawning the runtime,
but that is a small change for a meaningful safety improvement.

### S3 — C2a 3-commit split introduces a dead-code commit (C2a-2)

**Problem.** Commit `57d043e` (C2a-2) adds `podman.rs` and `report.rs` but no
caller that exercises them. The `podman_run` and `build_report` round-trip logic
land without the `runner::run` that is their first caller (which comes in
C2a-3). This violates the git-commits rule: "every function, struct, and field
added in this commit has at least one caller or reader in the same commit."

The split was motivated by size control — C2a-1 (1510 lines) and C2a-2 (519
lines) together total 2029 lines before C2a-3 (1474 lines) arrives. But the
split point creates a dead-code commit: C2a-2's `podman_run` and `podman_stop`
have no callers until C2a-3.

**Impact.** The git-commits smell test fails on C2a-2: no callers in the commit.
This isn't a production bug, but it makes the commit boundary hard to reason
about and means C2a-1 and C2a-2 cannot be independently reverted without leaving
broken intermediates. For a 5-commit phase this is minor, but the review
criterion flags it.

**Recommendation.** Acknowledged as a sizing trade-off. A cleaner split would
have been (1) types + config/secrets + podman/report primitives with a
`#[cfg(test)]` caller, (2) runner + in-container builder + CLI. For a future
phase, prefer keeping primitive + its first caller in the same commit even if
that commit is large, rather than splitting at the primitive boundary.

### S4 — `podman stop --all` reachable from `podman_stop(None)`

**Problem.** `podman.rs:build_stop_argv` passes `name.unwrap_or("--all")` when
`name` is `None`. This is the `podman stop --all` form. Design 009 is explicit:
"009 never uses the wrapper's `name = None` form (which maps to
`podman stop --all` and would tear down unrelated containers)." The runner
itself always passes `Some(ctr_name)`, so the `None` path is never reached from
`runner::run`. However, `podman_stop` is `pub`, and the `None` form maps to
tearing down all running containers on the host — a footgun waiting for an
accidental `None` at a future callsite.

**Impact.** Any future call to `podman_stop(None, _)` from any code path nukes
all running containers on the host, including containers unrelated to the CBS
build. There is no type-level barrier to this.

**Recommendation.** Either: (a) make the `None` → `--all` behavior private and
unexported, keeping only
`pub async fn podman_stop(name: &str, timeout: Duration)` with a required name;
or (b) split into two functions:
`podman_stop_by_name(name: &str, timeout: Duration)` (public) and
`podman_stop_all(timeout: Duration)` (private or crate-internal), so the "stop
all" path is never accidentally reachable from a `None`. Option (a) is
sufficient for current needs.

---

## Minor Observations

### M1 — `subprocess.rs:expect` panics on `stdout`/`stderr` take

`subprocess.rs:111-112` uses `expect("stdout was piped")` and
`expect("stderr was piped")`. These cannot actually fire — `Stdio::piped()` was
set on lines 108 and the `child` was spawned successfully — but `expect` still
constitutes a panic in library code. The invariant is guaranteed by construction
(piped immediately before spawn); a comment explaining why makes this
acceptable. A `map_err(|_| CommandError::Io(...))` replacement would be
idiomatic but is not required.

### M2 — `cosign_already_installed` stderr match uses `contains`, Python used `re.match`

`builder/mod.rs:191` checks `out.stderr.contains("already installed")`. Python's
`prepare.py:117` used `re.match(".*already installed.*", stderr)`. `re.match`
anchors at the start unless `.*` is prepended, which it is here, so the Python
check is equivalent to a substring match. The Rust `contains` is semantically
identical. No issue, but worth noting for future parity audits.

### M3 — `write_log` concatenates stdout+stderr in that order without a separator

`runner.rs:425` writes `format!("{}{}", output.stdout, output.stderr)` to the
log file. If both streams have content, they are concatenated without a
delimiter, which may confuse operators reading the log. Python used a per-line
streaming callback (`log_cb`) that interleaved lines as they arrived. The
current collected form (before live streaming lands in C3) is a noted
placeholder; just confirm that the real streaming callback path in a later
commit replaces this whole block, not augments it.

### M4 — `versions/create.rs` change is in the C2a-3 commit

The `F2` fix is in its own commit (`ceaff98`), but `versions/read.rs` (the new
component loader) was added in C2a-3 (`b3cce70`). The F2 commit correctly
updates `components.rs` and `versions/create.rs`. Both commits compile and test
independently. No issue — the split is clean.

### M5 — `VaultGpgPvtKey` field `private_key` is required but semantically it is the key path

In `secrets.rs:VaultGpgPvtKey`, `private_key: String` is a required field
alongside `key: String` (the Vault path). Looking at the Python source
(`models.py`), `VaultGpgPvtKey` has both `key` (the Vault path) and
`private_key` (the key identifier or path within Vault). The naming is faithful
to Python, but the field purpose is ambiguous. A doc comment clarifying what
`private_key` represents in the Vault variant (is it a hint? a fallback path?)
would prevent future misuse.

### M6 — `gen_run_name` has a 1-in-26^10 collision risk in concurrent tests

`runner.rs:gen_run_name` generates `ces_<10-random-lowercase>`. The end-to-end
tests in `runner_podman.rs` use
`format!("cbscore-runner-ok-{}", std::process::id())` instead, which is correct.
But if `gen_run_name` were used in parallel integration tests (e.g., when the
worker calls it), a collision is theoretically possible. This is not an issue
today but note it for C4a when the worker integration lands.

---

## Strengths

- **Invariant 5 (report-before-rc)**: `runner.rs:220-231` reads the report,
  writes the log, and only then checks `output.code != 0`. The partial-report is
  correctly threaded into `RunnerError::NonZeroExit { report, stderr }`. This
  exactly fixes the Python bug documented in design 009 and is tested by the
  `a_nonzero_exit_carries_the_partial_report` integration test.

- **Staging dir RAII**: `tempfile::TempDir` (a RAII guard) is the sole owner of
  all temp inputs including the plaintext secrets file. It drops on every return
  path — success, `NonZeroExit`, `Podman`, `Cancelled`, and any early-return IO
  error. The Python leak (temp secrets file left on `PodmanError`) is genuinely
  fixed.

- **Cancellation model**: `runner.rs:207-214` uses `select!` with the
  `CancellationToken`, calls `podman_stop(Some(&ctr_name))` (never
  `None`/`--all` from the runner itself), and returns `RunnerError::Cancelled`.
  No async Drop reliance. Exactly matches design 009's specification.

- **Mount table and argv parity**: `build_run_args` in `runner.rs` matches the
  Python `podman_volumes` and `podman_args` exactly, including the `:Z` SELinux
  relabel on `scratch_containers`, the `/dev/fuse:rw` device,
  `use_host_network`, `unconfined`, no `use_user_ns`. The unit test
  `run_args_build_the_full_mount_and_argv` pins this in detail and matches
  design 009's mount table row by row.

- **`out_cb` design rationale**: the deviation from Python (collect AND stream
  vs. collect OR stream) is concretely justified — retained stderr on failed
  toolchain steps — and is clearly documented in three places: the module doc,
  the `RunOpts.out_cb` field comment, and the commit message. The rationale is
  sound.

- **F2 fix**: `CoreComponent` now requires `build` and `containers` (via serde
  field presence), so a manifest missing either fails to deserialize and is
  error-skipped. The `the_rpm_section_is_optional` test confirms the intended
  leniency on `rpm` is preserved. The integration test in `versions_create.rs`
  was correctly updated to use a complete component manifest.

- **prepare_steps parity**: the toolchain install sequence in `builder/mod.rs`
  is a faithful port of `prepare.py`'s five-step sequence, and the unit test
  `prepare_steps_match_the_python_toolchain_sequence` locks it against drift.
  The cosign already-installed case is handled correctly.

- **`absolutize` is correct**: `std::path::absolute` (not `canonicalize`) is
  used, which does not require the path to exist — correct for `ccache` which
  may not exist yet. This matches Python's `.resolve()` (which also handles
  nonexistent paths on Python 3.6+).

- **Commit message quality**: all five commit messages clearly describe _why_
  the change exists (not just _what_), call out deliberate divergences from
  Python and from the designs, and are appropriately sized. The F2 commit
  message correctly names the review finding it addresses.

---

## Open Questions

1. `apply_home_hook` is scoped to the `runner build` entry. Does the binary also
   run as PID 1 on any other entrypoint path (e.g., a future `build` invoked
   directly inside the container)? If so, the hook needs to be applied there
   too, or factored out.

2. The `write_log` in `runner.rs` writes collected stdout + stderr after the
   container exits. Design 009 says the log sink is "file path XOR callback" and
   the live streaming path ("the worker's callback / the CLI's stdout sink")
   lands "in a later commit." Confirm that when C3 lands the live-streaming
   path, the `write_log` fallback is removed rather than kept alongside, to
   avoid writing the log twice.

3. `subprocess.rs:stream_and_collect` feeds both stdout and stderr through the
   same `out_cb`. In C2b's `prepare_builder`, this means dnf progress output on
   stderr and stdout are interleaved through the same log callback. Is that the
   intended behavior, or should stdout and stderr be tagged differently for the
   operator-visible log?

4. The cosign RPM is x86_64-only (`cosign-2.4.3-1.x86_64.rpm`). Is aarch64
   support required? The CI matrix (verified in memory: x86_64 and aarch64 musl
   targets) suggests it might be.

---

## Confidence Score

| Item                                                        | Points | Description                                                                                                                                                                                      |
| ----------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Starting score                                              | 100    |                                                                                                                                                                                                  |
| D7: Debug on secret-value enums                             | -20    | `GitSecret`, `StorageSecret`, `SigningSecret`, `RegistrySecret` all `#[derive(Debug)]`; plaintext fields exposed via `{:?}`. Invariant 4 violation.                                              |
| D8: out_cb spec deviation not reflected in design 003       | -5     | Design 003 says empty strings when callback set; code does the opposite and is better, but the design hasn't been amended.                                                                       |
| D12: C2a-2 is a dead-code commit                            | -20    | `podman_run`/`podman_stop`/`podman.rs` land with no callers until C2a-3. Fails the no-dead-code smell test.                                                                                      |
| D7: unsafe set_var inside tokio runtime                     | -10    | Safety comment is incorrect (Tokio workers are running); technically UB though practically benign in this binary today. Treated as D7 (security/correctness gap, not D4) because it is `unsafe`. |
| D9: write_log stdout+stderr concatenation without separator | -5     | Produces ambiguous log files; placeholder until live streaming lands, but no comment marks it as temporary.                                                                                      |
| **Total**                                                   | **40** |                                                                                                                                                                                                  |

Score: **40 / 100**

The D12 deduction (dead-code commit) and D7 deductions (Debug on secret types +
unsafe set_var) together dominate. The D12 is a commit-hygiene finding that does
not affect correctness; the two D7 findings represent real risks (invariant 4
leak path; undefined behavior in the in-container binary). The D8 is a
documentation debt that will confuse future reviewers. The D9 is a temporary gap
that will be resolved by a later commit.

---

## Verdict: Approve with conditions

The five commits deliver correct, well-tested keystone functionality. No
production correctness bugs or data loss risks were found. Before the next
commit (C3) proceeds:

**Required (before C3):**

1. Remove `#[derive(Debug)]` from the four live-secret enums in `secrets.rs` and
   implement redacting `Debug` impls. C3 adds `SecretsMgr` callers that will
   naturally format error context containing these types.

2. Amend design 003 to document the decided `out_cb` collect-AND-stream behavior
   (a one-sentence change in the "Behavior" section).

**Recommended (before C3, but not blocking):**

3. Move `apply_home_hook` before the Tokio runtime starts (restructure `main.rs`
   to detect `runner build` before `tokio::main`). This makes the safety
   invariant actually verifiable rather than asserted in a comment.

4. Make `podman_stop(None, _)` either private or replace it with a type-safe
   named-only variant to prevent accidental `--all` usage from future callsites.

Items 3 and 4 are not blockers for C3 but should be closed before the worker
integration (C4a/M4) which adds new callers.
