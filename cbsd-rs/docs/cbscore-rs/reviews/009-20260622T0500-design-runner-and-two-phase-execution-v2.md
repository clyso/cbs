# Review — 009 Runner & two-phase execution (v2)

Second-round adversarial design review of
`cbsd-rs/docs/cbscore-rs/design/009-20260622T0439-runner-and-two-phase-execution.md`,
after the v1 (37/100, go-with-changes) findings were folded in. This pass (a)
confirms each v1 finding is genuinely resolved in the **current** doc — not
merely reworded — and (b) independently hunts for issues the edits introduced or
that v1 missed.

Every resolution claim was re-verified line-by-line against the Python source of
truth, not against the doc or v1: `cbscore/src/cbscore/runner.py`,
`cbscore/src/cbscore/_tools/cbscore-entrypoint.sh`,
`cbscore/src/cbscore/utils/podman.py`, `cbscore/src/cbscore/cmds/builds.py`. The
chartering doc 001 (B2, PID-1 invariant) and the in-container half 007
(`Builder.run`) were read for ownership/consistency, not relitigated. The
settled fix-not-port decisions (HOME conditional, partial-report carry, RAII
secrets cleanup) are assessed for **coherence and ownership**, not Python
fidelity.

## Verdict

**go-with-changes.** The four substantive v1 findings are genuinely resolved:
the cancellation mechanism is now self-contained and coherent (the circular
009↔003 punt is gone), HOME is conditional, the partial report rides the failure
path, and the RAII cleanup covers the plaintext secrets file on every path
including `PodmanError`. The score rises from 37 to 66.

What remains are three **gaps in the report-carry fix the edits introduced** —
all cheap wording/specification fixes, all able to mislead an implementer — plus
one new ownership gap and several carried-over v1 minors that were never
addressed:

1. **The Errors section was not updated for the report-carry fix.** The
   round-trip section says the failure `RunnerError` carries
   `Option<BuildArtifactReport>`; the canonical Errors section — the part an
   implementer reads to define the type — still describes it as carrying the
   captured stderr and omits the report payload. The fix landed in one section
   and never reached the type-defining one.
2. **The report-carry mechanism is offered as two alternatives, one of which
   does not typecheck** against the stated `run` signature.
3. **The report-carry fix is unspecified on the cancel/timeout/podman-error
   paths** — the read is anchored "before the rc check," but those paths never
   produce an rc, so the read does not run there even though the design claims
   they carry the report.
4. **The conditional-HOME-at-startup requirement has no owning document** and
   its "as PID 1" framing is unexamined for the host-CLI case (the same binary
   is also the host CLI).

These block a clean "go" only because an implementation contract must not
contain a self-contradiction, an incoherent alternative, or an unreachable
guarantee. They are paragraph-level fixes, not rework.

## Part 1 — v1 finding resolution (verified against source)

### 1. Cancellation mechanism — RESOLVED (coherent, self-contained)

v1's headline finding: 009 punted external-token handling to 003, which punted
back to 009; 001 B2 chartered 009 to own the observe-token + stop-by-name
mechanism, and it did not.

The current §"Timeout & cancellation" (009 lines 127–140) now states the
mechanism in full and owns it:

> 001 B2 picks the mechanism and delegates the detail here, so **009 owns it** …
> `runner::run` `select!`s the `podman_run` future against two triggers:
> `tokio::time::timeout(opts.timeout)` and the caller-supplied
> **`CancellationToken`** … the runner **explicitly calls
> `podman_stop(name = ctr_name)`** … because dropping the `podman_run` future
> runs no async cleanup (Rust has no async `Drop`).

This matches what 001 B2 charters (001 lines 117–121: "the runner observes …
**before** returning, awaits `podman stop <ctr_name>` … no async `Drop`"). The
"`podman_run` already does this on its own timeout (003)" framing that v1 called
circular is gone; 003 is now cited only as the source of the `podman_stop`
primitive, not as the owner of the mechanism. The fidelity-notes line (195–197)
restates the same self-contained mechanism. The Python behavior this replaces is
accurately characterized: `builds.py:207–213` catches `KeyboardInterrupt` and
calls `task.cancel()`, which propagates `CancelledError` into `podman_run`'s own
handler (`podman.py:118–126`, cidfile→`podman_stop(name=cid)`) — a different
(internal-await) trigger, which 009 correctly no longer conflates with the
external token.

**Status: RESOLVED.** Self-contained and coherent. See NEW finding N5 for an
unreconciled double-timeout introduced alongside this text, and N3 for how this
path's report read interacts with the report-carry fix.

### 2. HOME — RESOLVED (conditional, correctly attributed)

v1: 009 set `-e HOME=/runner` unconditionally and mis-described the reference
(`entrypoint.sh`) as unconditional.

The current doc (009 lines 112–119, 187–189) now states HOME is **not
host-set**; instead `cbsbuild` as PID 1 replicates the conditional "set
`HOME=/runner` iff unset or `/`", and explicitly notes "An unconditional
`-e HOME=/runner` would wrongly override an image's `/root`." This matches
`entrypoint.sh:19–22`:

```bash
if [[ -z ${HOME} ]] || [[ ${HOME} == "/" ]]; then
  HOME="${RUNNER_PATH}"
  export HOME
fi
```

and is consistent with `runner.py:290–294`, whose host env block sets only
`CBS_DEBUG`. The contradiction v1 flagged ("unconditional" vs. the
half-admission it was conditional) is gone.

**Status: RESOLVED** as a coherence matter. The fix relocated the behavior into
the binary — see NEW finding N4 for the resulting ownership gap (which doc
specifies that startup behavior, and what happens on the host-CLI path).

### 3. Partial report on the failure path — RESOLVED (as a deliberate fix)

v1: the doc claimed the partial report is "captured for telemetry / reproduced
exactly," but Python discards it on `rc != 0`.

The current §"Build-report round-trip" (009 lines 148–157) now labels this a
**fix of broken Python**, accurately quotes the Python bug, and states the
report rides the failure path. Verified against `runner.py:318–340`: the report
is read at 322–324, unlinked at 333, then on `rc != 0` (335–338) a `RunnerError`
is raised carrying only `stderr` — the parsed `report` local is discarded. The
doc's characterization ("the read-before-rc is dead effort") is correct. The
"reproduced exactly" mislabel is gone; it is now framed as an improvement.

**Status: RESOLVED** as a coherence/labeling matter. But the fix's mechanism is
internally inconsistent across two sections, offers an incoherent alternative,
and is unspecified on the cancel/timeout/podman-error paths — see NEW findings
N1, N2, and N3. The _intent_ is resolved; the _specification of the intent_ is
not yet clean.

### 4. Temp-file leak (plaintext secrets) — RESOLVED (RAII on every path)

v1: 009 labeled the cleanup as parity; Python leaks the secrets and config temp
files on the success and `PodmanError` paths.

The current doc covers this in three places — step 7 (009 lines 68–72: "clean up
**all** temp inputs … on **every** path via RAII guards"), the fidelity note
(198–202), and the testing section (228–230, "no plaintext-secret file is left
on disk … incl. `PodmanError`"). It explicitly names the secrets file as
plaintext creds and cites `runner.py:286–316`.

Verified against `runner.py`: `secrets_tmp_path` is created at 216 and unlinked
**only** at 222 (`ConfigError`/`SecretsError`) and 250 (config-store error);
`new_config_path` is created at 246 and unlinked **only** at 251; the `finally`
at 315–316 runs `_cleanup_components_dir` and nothing else. So on the success
path and the `PodmanError` path (caught at 307–310, then the components-only
`finally`), both temp files survive — `secrets_tmp_path` being plaintext secrets
written by `secrets.store` at 220. The doc's description of the leak is exact,
and the RAII-on-every-path remedy genuinely covers the `PodmanError` path (an
RAII guard's `Drop` runs on unwind/early-return regardless of which error
fired). The testing section asserts the `PodmanError` path explicitly.

**Status: RESOLVED.** The remedy is sound and covers the security-relevant path.

### 5. `/runner/logs` rewritten but never mounted — RESOLVED (flagged)

v1: the doc reproduced the logging-path rewrite without noting `/runner/logs` is
never mounted.

The current doc drops the rewrite and adds a fidelity note (009 lines 63,
203–206) explaining Python rewrites `config.logging` to
`/runner/logs/cbs-build.log` but never mounts `/runner/logs`, so the
in-container log is ephemeral; host capture is the streamed callback. Verified:
`runner.py:241` sets the logging path; `podman_volumes` (257–266) has no
`/runner/logs` entry; host logging is the `output_cb`→`_log_callback` path
(`runner.py:78–106`, 287). Accurate.

**Status: RESOLVED**, with a small residual ambiguity about what the
in-container `config.logging` field resolves to once the rewrite is dropped —
see NEW finding N6 (minor).

## Part 2 — NEW findings (introduced by the edits or missed by v1)

### N1 (high) — Errors section omits the report-carry fix

The §"Build-report round-trip" (009 lines 154–157) says the failure-path error
now carries the partial report: "the `RunnerError` carries an
`Option<BuildArtifactReport>` … so … the worker can record what was built before
the failure." But the §"Errors" (009 lines 173–178), describing the same
non-zero-exit case, still says `RunnerError` wraps "a non-zero container exit
(**carrying the captured stderr**)" — with no mention of the report payload. It
does not assert the report's absence, so this is an omission rather than a flat
self-contradiction; but §Errors is the canonical, type-defining section an
implementer reads to shape `RunnerError`, and it is incomplete. As written, the
two sections describe two different `RunnerError` shapes, and the one chartered
to define the type is the one missing the payload.

_Change:_ update §Errors to state the non-zero-exit variant carries both the
captured stderr **and** `Option<BuildArtifactReport>`, matching the round-trip
section.

### N2 (high) — report-carry offered as two alternatives, one of which does not typecheck

009 line 154–157 hedges the mechanism: "the `RunnerError` carries an
`Option<BuildArtifactReport>`, **or** `run` returns the report alongside the
error." The `run` signature (009 line 42) is
`Result<Option<BuildArtifactReport>, RunnerError>`. A `Result` is `Ok` **xor**
`Err`; the error arm cannot also "return the report alongside" — the only way to
convey a report on the failure path with that signature is to carry it
**inside** the `RunnerError` variant. The second alternative is incoherent with
the doc's own signature. An implementation contract must commit to one
mechanism, not offer a self-excluding pair.

_Change:_ drop the "or `run` returns the report alongside the error"
alternative; commit to the report living inside the `RunnerError` variant (which
N1 also requires). If instead the intent were to widen the success type, the
signature on line 42 would have to change — but that contradicts "on a non-zero
exit … return `RunnerError`" (line 71), so the in-error-variant form is the only
one consistent with the rest of the doc.

### N3 (medium) — the cancellation and report-carry fixes interact incoherently on the cancel/timeout/podman-error path

The two headline fixes were specified independently, and their interaction is
unresolved. The report read is anchored to a normal `podman_run` return: step 6
(009 lines 66–67) reads `build-report.json` "**before** the return-code check,"
i.e. at a point where `podman_run` returned an `rc`. But on the cancel/timeout
path the `select!` (009 lines 130–140) **drops** the `podman_run` future and
calls `podman_stop` — `podman_run` never returns an `rc`, so the step-6 read, as
anchored, does not run. Likewise on the `PodmanError` path `podman_run` raises
rather than returning an `rc`. Yet step 7 (009 lines 71–72) claims these paths
"return `RunnerError` (carrying any partial report)." The design has not decided
whether the runner reads the scratch report **after** `podman_stop`/error on
those paths (it can — the report is a file on the host scratch mount and
survives container death, 009 line 66) or whether "carrying any partial report"
is simply unreachable there. This mirrors Python exactly: `PodmanError` at
`runner.py:307–310` short-circuits **before** the read at `runner.py:318`, so
Python never carries a report on that path. Because "the report rides the
failure path" is the v1-finding-3 resolution claim, the fix only half-holds: it
covers the ordinary non-zero-`rc` exit but is unspecified on the
cancel/timeout/podman-error exits.

_Change:_ state that the runner attempts the scratch-report read on **every**
exit path that produces a `RunnerError` — after `podman_stop` on cancel/timeout
and after catching a `PodmanError` — not only after a normal return-code, so
"carrying any partial report" holds uniformly. (If the report is intentionally
not carried on those paths, say so and drop the "(carrying any partial report)"
qualifier from step 7 for them.)

### N4 (medium) — conditional-HOME-at-startup has no owning doc and an unexamined host-CLI case

The HOME fix (finding 2 above) relocates the conditional "set `HOME=/runner` iff
unset or `/`" from the host into `cbsbuild` "as PID 1" (009 lines 112–119,
187–189). Two gaps:

- **No owner.** 009 delegates the startup behavior to the binary but does not
  own it (correctly — 009 is host-side). The in-container CLI shape is owned by
  010, which **does not yet exist** (no `010-*` design file is present;
  `design/` ends at 009). 007 (`Builder.run`, the in-container half) has **no**
  mention of HOME, startup, or PID 1 (grep: zero hits). 001 establishes
  "`cbsbuild` runs as PID 1" generically (001 lines 71, 88, 94, 188, 291) but
  says nothing about HOME. So the one behavior that replaces the entrypoint's
  conditional currently lives in no document's contract — it can fall through
  the cracks between 009 and the not-yet-written 010.
- **Host-CLI path.** The same musl `cbsbuild` binary is also the host-side CLI
  (009 line 17; 001 B1 binary-mount). A blanket "at startup, set `HOME=/runner`
  iff unset or `/`" would also fire when the binary runs as the host CLI, where
  `/runner` does not exist. "as PID 1" is doing unexamined work: the doc does
  not say how the binary distinguishes "I am container PID 1" from "I am the
  host CLI," nor that the HOME logic is gated to the in-container `runner build`
  path only.

_Change:_ add a forward-reference note that 010 (in-container CLI) must specify
the conditional-HOME-at-startup and gate it to the in-container entry (e.g. only
under `runner build`, or only when `/runner` exists), so it never fires on the
host CLI. This keeps 009 host-side while ensuring the behavior is owned
somewhere.

### N5 (medium) — double timeout: podman `--timeout` and the outer `select!` are unreconciled

`opts.timeout` now feeds **both** podman's `--timeout` flag (009 line 123, flags
table) **and** the outer `select!`'s `tokio::time::timeout(opts.timeout)` (009
line 130). The doc does not reconcile them. They are different mechanisms with
different outcomes:

- If podman's own `--timeout` fires first, `podman_run` returns a normal
  non-zero `rc` (podman kills the container and exits), so `runner::run` takes
  the ordinary build-failure path and produces a generic non-zero-exit
  `RunnerError` — **not** the timeout/cancel `RunnerError` the testing section
  expects (009 line 227: "elapsing the timeout … returns a timeout/cancel
  `RunnerError`").
- If the outer `select!` timeout fires first, the runner calls
  `podman_stop(name = ctr_name)` and returns the timeout/cancel error.

With both set to the same value the winner is a race, so the same elapsed
timeout can yield either error shape. The double timeout is itself **inherited
from Python**, not introduced by the edits: `podman_run` already passes
`timeout` to **both** the `--timeout` CLI flag (`podman.py:77–78`) **and**
`async_run_cmd(cmd, timeout=timeout, …)` (`podman.py:115`) — and it is the
asyncio one that fires the cancel handler at 118–126. The new issue is that 009
moved the asyncio half out to the runner's own `select!` while leaving podman's
`--timeout` in the flags table, and the testing assertion (009 line 227) assumes
the `select!` branch always wins.

_Change:_ pick one owner. Either drop podman's `--timeout` (the runner's
`select!` now owns the timeout, and `podman_stop` is the only stop path), or
keep `--timeout` as a belt-and-suspenders backstop set to a value strictly
greater than the `select!` timeout, and state that the `select!` is the primary
path. Reconcile the testing assertion accordingly.

### N6 (low) — dropped logging rewrite: in-container `config.logging` left unspecified

The port drops Python's `config.logging` → `/runner/logs/cbs-build.log` rewrite
(009 lines 63, 203–206). The doc says it "omits the redundant rewrite" but does
not say what the cloned container config's `logging` field then **is**. Python
only ever set `new_config.logging` when `log_file_path` was truthy
(`runner.py:237–241`); otherwise the field carried whatever `config.logging`
held on the host. If the Rust port likewise just omits the rewrite, the
container config inherits the host `log_file` path (a host path that need not
exist in-container). The config-rewrite section (009 lines 56–63) lists every
other rewritten field but is silent on `logging`.

_Change:_ state explicitly whether the container config's `logging.log_file` is
**cleared/disabled** (preferred — the streamed callback is the real capture) or
left to inherit, and add it to the rewrite list for completeness.

### Carried-over v1 minors still unaddressed (low)

These v1 minors were not addressed in the current doc (grep for
`distro`/`--all`/`symlink`/`image=`: zero hits):

- **Launched image is `desc.distro`** (`runner.py:289`) — 009 still never states
  which image is launched.
- **`stop(name=None)` → `podman stop --all`** (`podman.py:131–134`) — a no-name
  stop stops every container on the host. 009's cancellation always passes
  `name = ctr_name`, but the doc still does not note whether the Rust `stop`
  keeps or guards the `--all` fallback — a footgun for the in-process worker.
- **Symlink-entrypoint check dropped** (`runner.py:187–190`) — step 1 (009 lines
  47–50) validates exists+executable only; the dropped symlink rejection is
  still unacknowledged. Likely fine (the artifact is now a trusted image-shipped
  binary) but should be stated.

(v1's fourth minor, `gen_run_name` PRNG→CSPRNG, **is** now addressed at 009
lines 161–164, flagged as collision-avoiding, not security-sensitive.)

## Part 3 — re-confirmed faithful items (still correct)

Spot-checked against source; v1's "faithful" verdicts still hold in the current
text:

- **Mount table & flags** — `:Z` on `/var/lib/containers` (`runner.py:264` → 009
  line 108), `/dev/fuse` device (297 → 120), `--security-opt label=disable` +
  `seccomp=unconfined` (`podman.py:60–81` → 123), `--network host` (302 → 123),
  no user namespace (`use_user_ns=False`, 300 → 123),
  `--cidfile`/`--name`/`--timeout`/`--replace` (282/299/301/304 → 123).
- **In-container argv** —
  `--config … runner build --desc … --tls-verify=… [--skip-build] [--force]`
  (009 lines 89–92) matches `entrypoint.sh:57–59` + `runner.py:256,276–280`,
  with `--debug` correctly replaced by `CBS_DEBUG` per H3.
- **Config rewrite** — `secrets`, `vault` (only if set), `scratch`,
  `scratch_containers = /var/lib/containers`, `components`, `ccache` (only if
  set) match `runner.py:229–235` (009 lines 56–62). (See N6 for the `logging`
  field.)
- **`CBS_DEBUG=<1|0>`** — `runner.py:290–294` (effective level == DEBUG),
  forwarded as env per H3 (009 line 112, 185).
- **Binary mount / no entrypoint** (B1) — settled; not relitigated.

## Confidence score

Design-review adaptation of confidence-scoring. D8 = a fidelity/coherence claim
that deviates from source or contradicts another section of the same doc; D1 = a
behavior the design delegates but that has no owning document; D11 = a missing
note for a real quirk. The four big v1 deductions (cancellation −20, HOME −10,
partial-report −10, secrets-leak −10) are **resolved** and do not recur.

| Item                                                                        | Points | Description                                                                                                             |
| --------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                              | 100    |                                                                                                                         |
| D8: type-defining Errors section omits report-carry fix (N1)                | -8     | §Errors describes non-zero exit as carrying stderr, omits the report; §round-trip carries `Option<BuildArtifactReport>` |
| D8: report-carry offered as two alternatives, one incoherent (N2)           | -6     | "or `run` returns the report alongside the error" cannot hold for `Result<Option<_>, RunnerError>` (line 42)            |
| D8: report-carry unspecified on cancel/timeout/podman-error paths (N3)      | -4     | read anchored "before the rc check" (step 6) never runs when no rc is returned; step 7 still claims report is carried   |
| D1: conditional-HOME-at-startup has no owning doc; host-CLI case (N4)       | -5     | delegated to 010 (absent) / "as PID 1"; 007 silent; same binary is the host CLI and the gating is unexamined            |
| D8: double timeout (podman `--timeout` + `select!`) unreconciled (N5)       | -3     | same `opts.timeout` feeds both; podman-win path yields a generic, not timeout, `RunnerError`                            |
| D11: dropped logging rewrite leaves in-container `logging` unspecified (N6) | -2     | doc omits rewrite but never says the field is cleared vs. inherited (`runner.py:237–241`)                               |
| D11: launched image `desc.distro` still unstated                            | -2     | `runner.py:289` — carried over from v1, unaddressed                                                                     |
| D11: `stop(name=None)`→`--all` footgun still unstated                       | -2     | `podman.py:131–134` — carried over from v1, unaddressed                                                                 |
| D8: symlink-entrypoint check still dropped, unacknowledged                  | -2     | `runner.py:187–190` — carried over from v1, unaddressed                                                                 |
| **Total**                                                                   | **66** |                                                                                                                         |

Interpretation: 66 sits in "significant issues; address before proceeding." This
is a large improvement over v1's 37: every structural and security-relevant v1
finding is genuinely resolved (not merely reworded), and the design is faithful
where it claims fidelity. The residual is dominated by three gaps in the
report-carry fix (N1 omission, N2 incoherent alternative, N3 unspecified on the
cancel/error paths) — all wording-level but contract-breaking — plus an
ownership gap (N4) and minor carryovers. Read the score as: **the design is one
editing pass from "go"** — pin down the `RunnerError` shape once and the
report-read on every exit path (N1/N2/N3), give the HOME-at-startup behavior an
owner (N4), and the carried-over minors are one-line notes.

## Per-finding resolution status (v1 → v2)

| v1 finding                                         | v2 status               | Evidence                                                                                |
| -------------------------------------------------- | ----------------------- | --------------------------------------------------------------------------------------- |
| Cancellation mechanism (circular 009↔003 punt)     | RESOLVED                | 009 §Timeout&cancellation 127–140 owns `select!`+`podman_stop(name)`; 001 B2 117–121    |
| HOME unconditional / mis-attributed                | RESOLVED                | 009 112–119,187–189 conditional in `cbsbuild`; `entrypoint.sh:19–22` (see N4 owner gap) |
| Partial report discarded on `rc != 0`              | RESOLVED (intent)       | 009 148–157 labels it a fix; `runner.py:320–338` (but N1/N2/N3 — spec inconsistent)     |
| Temp secrets/config leak (success + `PodmanError`) | RESOLVED                | 009 68–72,198–202,228–230 RAII all paths; `runner.py:216,246,315–316`                   |
| `/runner/logs` rewritten but unmounted             | RESOLVED                | 009 63,203–206 drops rewrite + note; `runner.py:241` vs 257–266 (residual N6)           |
| Minor: launched image `desc.distro`                | UNRESOLVED (carry-over) | still unstated; `runner.py:289`                                                         |
| Minor: `stop(name=None)`→`--all` footgun           | UNRESOLVED (carry-over) | still unstated; `podman.py:131–134`                                                     |
| Minor: dropped symlink-entrypoint check            | UNRESOLVED (carry-over) | step 1 (009 47–50) exists+exec only; `runner.py:187–190`                                |
| Minor: `gen_run_name` PRNG→CSPRNG swap             | RESOLVED                | 009 161–164 flags it collision-avoiding, not security-sensitive                         |

## NEW findings ordered by severity

1. **N1 (high) — Errors section omits the report-carry fix.** The type-defining
   §Errors (009 173–178) still describes the non-zero-exit `RunnerError` as
   carrying the captured stderr and omits the report; §round-trip (154–157) says
   it carries `Option<BuildArtifactReport>`. Make §Errors carry both.
2. **N2 (high) — report-carry offered as two alternatives, one incoherent.** "or
   `run` returns the report alongside the error" (009 154–157) cannot hold for
   `Result<Option<_>, RunnerError>` (line 42). Commit to the in-error variant.
3. **N3 (medium) — report-carry unspecified on the cancel/timeout/podman-error
   paths.** The read is anchored "before the rc check" (step 6, 009 66–67), but
   those paths never return an rc (the `select!` drops `podman_run`;
   `PodmanError` raises). Step 7 (009 71–72) still claims they carry the report.
   Read the scratch report on every `RunnerError` exit, or drop the claim there.
4. **N4 (medium) — conditional-HOME-at-startup has no owning doc and an
   unexamined host-CLI case.** Delegated to absent 010 / "as PID 1"; 007 is
   silent; the same binary is the host CLI. Add a forward-ref that 010 owns it
   and gate it to the in-container path.
5. **N5 (medium) — double timeout unreconciled.** podman `--timeout` (009 123)
   and the `select!` timeout (130) both use `opts.timeout`; the podman-win path
   yields a generic, not timeout, error. Pick one owner.
6. **N6 (low) — dropped logging rewrite leaves in-container `logging`
   unspecified.** Say whether the field is cleared or inherited
   (`runner.py:237–241`).
7. **Carried-over v1 minors (low):** launched image `desc.distro` unstated;
   `stop(name=None)`→`--all` footgun unstated; dropped symlink-entrypoint check
   unacknowledged.
