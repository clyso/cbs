# Review — 009 Runner & two-phase execution (v3)

Third-round adversarial design review of
`cbsd-rs/docs/cbscore-rs/design/009-20260622T0439-runner-and-two-phase-execution.md`,
after the v2 (66/100, go-with-changes) findings N1–N6 plus carried-over minors
were folded in. This pass (a) re-verifies each v2 finding is genuinely resolved
in the **current** doc — not merely reworded — and (b) hunts specifically for
NEW inconsistencies the v2 edits introduced, since that exact failure mode (a
fix landing in one section but not propagating to the others) is what turned
v1's 37 into v2's new findings.

Every resolution claim was re-verified line-by-line against the Python source of
truth, not against the doc or the prior reviews:
`cbscore/src/cbscore/runner.py`,
`cbscore/src/cbscore/_tools/cbscore-entrypoint.sh`,
`cbscore/src/cbscore/utils/podman.py`, `cbscore/src/cbscore/builder/builder.py`.
The sibling designs 001 (B1/B2, invariants), 002 (`BuildArtifactReport`, error
taxonomy), 003 (`podman_run` / `podman_stop` and the subprocess primitive's
timeout), and 007 (`Builder.run`, when the report is written) were read for
cross-reference accuracy and ownership, not relitigated. Designs 010 and 011 do
**not** exist yet (`design/` ends at 009); any claim depending on them is
treated as unverifiable-by-design, not wrong.

## Verdict

**go-with-changes.** Every substantive v2 finding (N1–N6) and every carried-over
v1 minor is now genuinely resolved in the current text, and the resolutions are
accurate against the Python source. The report-carry mechanism is pinned to a
single in-error-variant form, the Errors section now defines it, the carry is
correctly scoped to the non-zero-exit path, HOME is conditional and
host-CLI-safe, the double timeout is collapsed onto `podman_run`, and
`config.logging` is cleared to `None`.

But the N5 timeout fix — like the v1 fixes before it — **did not propagate to
the Testing section**, producing a fresh internal contradiction, and the same
edit left the cancel path's error variant unsourced. These are the two NEW
findings:

1. **N7 (medium) — the Testing section contradicts the Timeout & cancellation
   section.** Testing asserts that elapsing the timeout triggers 009's explicit
   `podman_stop(name = ctr_name)`; the design body says timeout is wholly
   `podman_run`'s job and 009's `podman_stop` fires only on the cancel path.
2. **N8 (medium) — the cancel path returns `RunnerError::Podman(PodmanError)`,
   but on cancel the `select!` drops `podman_run`, so no `PodmanError` is ever
   produced.** The error has no source on that path.

Both are paragraph-level fixes, not rework. They block a clean "go" only because
an implementation contract must not contain a cross-section contradiction or a
guaranteed-but-unconstructable error value — the very standard the v2 round
applied to N1/N2. The score rises from 66 to 84.

## Part 1 — v2 finding resolution (re-verified against source)

### N1 — Errors section omits the report-carry fix — RESOLVED

v2: the round-trip section carried `Option<BuildArtifactReport>` but the
type-defining Errors section described the non-zero-exit variant as carrying
only stderr.

The current Errors section (009 lines 220–224) now defines:

> **`NonZeroExit { report: Option<BuildArtifactReport>, stderr: String }`** —
> the container ran to completion but exited non-zero. Carries the captured
> stderr **and the partial report** … This is the only report-bearing variant.

This matches the round-trip section's `Option<BuildArtifactReport>` (009 lines
182–186) and the `run` signature
`Result<Option<BuildArtifactReport>, RunnerError>` (009 line 42). The
type-defining section and the prose now agree.

**Status: RESOLVED.**

### N2 — report-carry offered as two incoherent alternatives — RESOLVED

v2: the round-trip text hedged "carries an `Option<BuildArtifactReport>`, **or**
`run` returns the report alongside the error," the second of which cannot hold
for a `Result`.

The current round-trip section (009 lines 181–186) commits to one mechanism:

> The port fixes this with a **single** mechanism: the non-zero-exit
> `RunnerError` variant **carries an `Option<BuildArtifactReport>`** … so the
> report rides exactly one of the two arms and never both.

The self-excluding "or `run` returns the report alongside" alternative is gone.
The mechanism is now stated once and is consistent with the line-42 signature.

**Status: RESOLVED.**

### N3 — report-carry unspecified on cancel/timeout/podman-error paths — RESOLVED (and accurate)

v2: the read was anchored "before the rc check," but the cancel/timeout/
podman-error paths never produce an rc, so the read does not run there even
though step 7 claimed they carry the report.

The current doc adds an explicit "Where the carry does and does not apply"
subsection (009 lines 188–200) that scopes the carry correctly:

- **Container ran and exited non-zero** — `podman_run` returned an rc; the
  report was read before the rc check; the carry is `Some`/`None`. "This is the
  path the fix targets."
- **Timeout / cancellation / podman failure** — `podman_run` raised instead of
  returning, so the read never executes and those variants hold no report.

This is **accurate against source**, on both halves:

- `runner.py:307–310` catches `PodmanError` and re-raises a `RunnerError`
  **before** the report read at `runner.py:318–333`. So Python never carries a
  report on the podman-error path — the read is unreachable there. The doc's
  claim matches.
- `builder.py` writes `build-report.json` **only** at line 134 (skipped path)
  and line 201 (full-build success). It is never written mid-flight, never on
  the `return None` no-upload path (line 184), and never on any `BuilderError`
  exception path. So a build killed by the deadline or the cancel token
  genuinely has no report on disk to recover — the doc's "no report exists"
  reasoning (009 lines 197–200) is correct, not merely asserted.

Step 7 (009 lines 76–79) now agrees: the timeout/cancel/podman-failure path
"returns `RunnerError::Podman` with no report (step 6 never ran)." The v2
inconsistency (step 7 claiming a carry there) is gone.

**Status: RESOLVED.**

### N4 — conditional-HOME-at-startup ownership & host-CLI case — RESOLVED

v2: the HOME conditional was relocated into the binary "as PID 1" with no owning
doc and an unexamined host-CLI case (the same binary is also the host CLI).

The current doc (009 lines 119–131, 245–249) now:

- States HOME is **not host-set** and that the conditional ("set `HOME=/runner`
  iff unset or `/`") moves into the binary, applied **only on the in-container
  `runner build` entry**.
- Explicitly handles the host-CLI case: "the host-side `cbsbuild build`
  invocation of the same binary runs in the operator's normal shell and must
  **not** touch `HOME`" (009 lines 128–131).
- Names the owner: "010 (CLI surface) owns wiring this startup hook onto the
  `runner build` subcommand" (009 lines 130–131).

The "unset or `/`" condition matches `entrypoint.sh:19–22` exactly, and
`runner.py:290–294` confirms the host env block sets only `CBS_DEBUG`. The 010
forward reference is unverifiable-by-design (010 does not exist yet) but is the
correct disposition for a host-side doc delegating an in-container behavior — it
is not a defect.

**Status: RESOLVED.**

### N5 — double timeout (podman `--timeout` + outer `select!`) — RESOLVED at the 009 contract level

v2: `opts.timeout` fed both podman's `--timeout` flag and an outer
`tokio::time::timeout` in 009's `select!`, with no reconciliation and a Testing
assertion that assumed the `select!` branch always won.

The current Timeout & cancellation section (009 lines 138–165) now states the
timeout is **wholly `podman_run`'s job**:

> 009 forwards `opts.timeout` and lets the wrapper own the dual
> `--timeout`/await-deadline behaviour. 009 does **not** add a second
> `tokio::time::timeout` around the call — that would be a third, redundant
> deadline racing the two `podman_run` already owns.

The 009-level `select!` now races `podman_run` against **only** the
`CancellationToken` (009 lines 152–156); the outer `tokio::time::timeout` v2
flagged is gone. The fidelity note (009 lines 255–260) restates the same split.

**The v2 factual claim is verified accurate.** Python passes `timeout` to BOTH:

- podman's own `--timeout` CLI flag — `podman.py:77–78`
  (`cmd.extend(["--timeout", str(int(timeout))])`).
- the `async_run_cmd` wait — `podman.py:115`
  (`await async_run_cmd(cmd, timeout=timeout, outcb=cb)`); it is this asyncio
  wait whose `CancelledError`/`TimeoutError` handler at `podman.py:118–126`
  reads the cidfile and calls `podman_stop(name=cid)`.

So the dual mechanism is genuinely inherited from Python, and 009 correctly
delegates it to `podman_run`/003 rather than reproducing it at the runner level.
The 009-level double-timeout contradiction is gone.

**Status: RESOLVED at the 009 contract level.** One residual remains (the dual
mechanism still lives inside `podman_run`/003 with a race-dependent outcome —
see N9, low) and the Testing section was not updated to match this fix (see N7,
medium — the new blocking finding).

### N6 — `config.logging` left unspecified after dropping the rewrite — RESOLVED

v2: the doc dropped Python's `config.logging` → `/runner/logs/cbs-build.log`
rewrite but never said what the field then is.

The current doc states `logging` is **cleared to `None`** in two places,
consistently:

- Step 4 (009 lines 62–64): "the port instead clears `logging` to `None` — see
  Fidelity notes."
- Fidelity note (009 lines 266–272): "instead **clears `config.logging` to
  `None`** in the path-rewritten container config (step 4) … the in-container
  `cbsbuild` then logs to stderr/stdout, which the host streams."

Both the rewrite step and the fidelity note now agree on `None`. Verified
against `runner.py:237–241` (Python only set `new_config.logging` when
`log_file_path` was truthy; otherwise the field inherited the host value) —
clearing to `None` is a clean, correct disposition that avoids the
inherit-a-host-path ambiguity.

**Status: RESOLVED.**

### Carried-over v1 minors — all RESOLVED

- **Launched image is `desc.distro`** — now stated: step 5 (009 lines 65–69)
  "launches **the image named by `desc.distro`** (the descriptor's distro tag,
  e.g. `rockylinux:9`)." Matches `runner.py:289` (`image=desc.distro`).
- **`podman_stop` always by name, never `--all`** — now stated: 009 lines
  162–165 "`podman_stop(name = ctr_name)` always stops **by name** here; the
  runner never uses the wrapper's `name = None` form (which maps to
  `podman stop --all` and would tear down unrelated containers)." Matches
  `podman.py:131–134`.
- **Dropped symlink-entrypoint check characterized as moot** — now stated:
  fidelity note (009 lines 238–242) "Python's entrypoint-script symlink/realpath
  check is **moot here** — the mounted artifact is an operator-shipped binary at
  a fixed path … not a script a build can swap." Correctly characterizes the
  `runner.py:187–190` check as moot under the binary-mount model.

**Status: all RESOLVED.**

## Part 2 — NEW findings (introduced by the v2 edits)

### N7 (medium) — Testing section contradicts the Timeout & cancellation section

This is the same failure mode v2 was chartered to catch: a fix landed in one
section and did not propagate to another. The N5 fix established (009 lines
138–165, 255–260) that the **timeout** is wholly `podman_run`'s job — on elapse
`podman_run` reads the cidfile and calls `podman_stop(name = cid)`
**internally** (`podman.py:118–126`; 003 lines 186–188), then raises
`PodmanError`. 009's own explicit `podman_stop(name = ctr_name)` is the
**cancel** path's action only (009 lines 152–156).

But the Testing section's Cancellation/timeout bullet (009 lines 291–293) still
asserts the pre-fix, conflated behavior:

> **Cancellation/timeout**: cancelling the token (or elapsing the timeout)
> triggers an explicit `podman_stop(name = ctr_name)` and returns a
> timeout/cancel `RunnerError` …

On the **timeout** path 009's explicit `podman_stop(name = ctr_name)` is never
called (the internal cidfile→`podman_stop(name = cid)` is, inside `podman_run`),
and the resulting error is `RunnerError::Podman`, not a distinct
"timeout/cancel" shape. The Testing bullet asserts the wrong mechanism for the
timeout half and re-conflates the two owners the design body spent a whole
section separating. This is a genuine internal contradiction in the
implementation contract: a test written to this bullet would assert a
`podman_stop(name = ctr_name)` call that the design says does not happen on
timeout.

_Change:_ split the Testing bullet into two assertions — (a) **cancellation**:
cancelling the token triggers 009's explicit `podman_stop(name = ctr_name)` and
returns the cancel error; (b) **timeout**: elapsing `opts.timeout` is handled
inside `podman_run` (internal cidfile→`podman_stop(name = cid)`) and surfaces as
`RunnerError::Podman`. No container is leaked on either.

### N8 (medium) — cancel path returns `RunnerError::Podman(PodmanError)` with no `PodmanError` source

Step 7 (009 lines 76–79) and the Errors section route cancel through
`RunnerError::Podman`:

- Step 7: "a **timeout/cancel/podman failure** returns `RunnerError::Podman`."
- Errors (009 lines 225–226): "`Podman(PodmanError)` — `podman_run` failed or
  raised on timeout/the cidfile→stop path."

For **timeout** and **podman-failure** this is right: `podman_run` raises
`PodmanError` and the runner wraps it. But on **cancel** the design is explicit
that the `select!` **drops** the `podman_run` future and the runner calls
`podman_stop(name = ctr_name)` itself (009 lines 152–156) — `podman_run` never
runs to completion, never returns, and never raises, so **no `PodmanError`
exists to wrap**. The variant `RunnerError::Podman(PodmanError)` requires a
`PodmanError` payload that the cancel path cannot construct.

This diverges from Python, where cancellation propagated _into_ `podman_run`'s
own handler (`builds.py` `task.cancel()` → `CancelledError` inside
`async_run_cmd` → `podman.py:118–126` → `PodmanError`), so Python always had a
`PodmanError` to wrap. The 009 design deliberately changed the trigger to an
external `select!` that drops the future (correct, per 001 B2 and the no-async-
`Drop` reasoning) — but that change means the cancel path no longer flows
through `podman_run`'s handler, so the error source it used to rely on is gone.
The doc did not reconcile the error variant with the new control flow.

_Change:_ specify what error the cancel path returns. Either add a dedicated
`RunnerError::Cancelled` variant (cleanest — it is semantically a different
outcome from a podman failure, and the worker/011 may want to distinguish a
user-initiated cancel from a tool failure), or state that the cancel path
synthesizes a `PodmanError` (e.g. `ETIMEDOUT`-style) after `podman_stop`. As
written, `RunnerError::Podman(PodmanError)` on cancel is unconstructable.

## Part 3 — residual observation (low, non-blocking)

### N9 (low) — the double timeout still lives inside `podman_run` with a race-dependent 009-visible outcome

N5 collapsed the 009-level double timeout but did not eliminate the dual
mechanism — it relocated it into `podman_run`/003, which still sets podman's
`--timeout` flag (`podman.py:78`) **and** wraps the wait in the primitive's
`tokio::time::timeout` (003 lines 70–76; `podman.py:115`). At the 009 contract
level this still has a race-dependent outcome on a single elapsed
`opts.timeout`:

- If podman's own `--timeout` fires first, podman kills the container and exits
  non-zero; `podman_run` **returns** a non-zero rc → step 6 reads the report →
  `RunnerError::NonZeroExit` (possibly with a partial report).
- If the primitive's await-deadline fires first, `podman_run` reads the cidfile,
  calls `podman_stop(name = cid)`, and **raises** `PodmanError` → step 6 never
  runs → `RunnerError::Podman` (no report).

So the same elapsed timeout can surface as either `NonZeroExit` or `Podman`
depending on which deadline wins. This is legitimately 003's mechanism (009
delegates to it correctly), and it mirrors Python — so it is a low-severity
residual, not a 009 defect. Worth one note, e.g. that `podman_run` should set
podman's `--timeout` strictly greater than the await-deadline so the
deterministic path is the await-deadline → `PodmanError`, and that 009 callers
should expect either outcome on timeout. If raised at all, this note belongs in
003 (which owns the dual mechanism), not 009.

## Part 4 — re-confirmed faithful items (still correct)

Spot-checked against source; the v1/v2 "faithful" verdicts hold in the current
text:

- **Mount table & flags** — `:Z` on `/var/lib/containers` (`runner.py:264` → 009
  line 115), `/dev/fuse` device (`runner.py:297` → 132),
  `--security-opt label=disable` + `seccomp=unconfined` (`podman.py:60–81` →
  136), `--network host` (`runner.py:302` → 136), no user namespace
  (`use_user_ns=False`, `runner.py:300` → 136),
  `--cidfile`/`--name`/`--timeout`/`--replace` (`runner.py:299,301,304` → 136).
- **In-container argv** —
  `--config … runner build --desc … --tls-verify=… [--skip-build] [--force]`
  (009 lines 96–99) matches `entrypoint.sh:57–59` + `runner.py:256,276–280`,
  with `--debug` replaced by `CBS_DEBUG` per H3.
- **Config rewrite** — `secrets`, `vault` (only if set), `scratch`,
  `scratch_containers = /var/lib/containers`, `components`, `ccache` (only if
  set) match `runner.py:229–235` (009 lines 56–62); `logging` now cleared to
  `None` (N6).
- **`CBS_DEBUG=<1|0>`** — `runner.py:290–294` (effective level == DEBUG),
  forwarded as env per H3 (009 lines 119, 243).
- **Report read-before-rc + unlink** — `runner.py:318–333`; report written
  in-container on the skipped (`builder.py:134`) and full-build
  (`builder.py:201`) paths only (007 line 192); 009 lines 167–186 reproduce the
  mechanics.
- **Temp-file RAII cleanup** (security fix) — Python leaks the secrets/config
  temp files on success and `PodmanError` (`runner.py:216,246` created;
  `finally` at 315–316 cleans only the components dir); 009 lines 70–79,
  261–265, 294–296 fix this with RAII on every path. Re-confirmed resolved.
- **Binary mount / no entrypoint** (B1), **gen_run_name PRNG→CSPRNG** (009 lines
  204–213, flagged collision-avoiding) — settled, not relitigated.

## Confidence score

Design-review adaptation of confidence-scoring. D8 = a fidelity/coherence claim
that deviates from source or contradicts another section of the same doc; D11 =
a missing note for a real quirk. The eight v2 deductions (N1 −8, N2 −6, N3 −4,
N4 −5, N5 −3, N6 −2, image −2, `--all` −2, symlink −2) are all **resolved** and
do not recur.

| Item                                                                  | Points | Description                                                                                                                                                          |
| --------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                        | 100    |                                                                                                                                                                      |
| D8: Testing section contradicts Timeout & cancellation (N7)           | -8     | Testing (009 291–293) asserts elapsing the timeout triggers 009's `podman_stop(name=ctr_name)`; body says timeout is `podman_run`'s job and that stop is cancel-only |
| D8: cancel path returns `Podman(PodmanError)` with no source (N8)     | -6     | `select!` drops `podman_run` on cancel (009 152–156), so no `PodmanError` exists to wrap; variant unconstructable                                                    |
| D11: relocated double timeout race-dependent 009-visible outcome (N9) | -2     | `podman_run`/003 still sets `--timeout` + await-deadline; same elapsed timeout → `NonZeroExit` xor `Podman`                                                          |
| **Total**                                                             | **84** |                                                                                                                                                                      |

Interpretation: 84 sits in "acceptable with noted improvements; fix before next
stage." This is a large improvement over v2's 66: every N1–N6 finding and every
carried-over minor is genuinely resolved (not reworded), and the resolutions are
accurate against the Python source — including the subtle ones (the report is
written only on the skipped/success paths, so "no report on a killed build" is
literally true; the dual-timeout claim is verified at `podman.py:78` and
`:115`). The residual is two NEW contradictions the N5 timeout fix introduced by
not propagating to the Testing section (N7) and not reconciling the cancel
path's error variant with the drop-the-future control flow (N8). Both are
paragraph- level. Read the score as: **the design is one editing pass from a
clean "go"** — split the Testing timeout/cancel assertions (N7), pin the
cancel-path error variant (N8), and optionally note the relocated dual timeout
in 003 (N9).

## Per-finding resolution status (v2 → v3)

| v2 finding                                            | v3 status  | Evidence                                                                                                |
| ----------------------------------------------------- | ---------- | ------------------------------------------------------------------------------------------------------- |
| N1: Errors section omits report-carry                 | RESOLVED   | 009 220–224 defines `NonZeroExit { report: Option<…>, stderr }`, "only report-bearing variant"          |
| N2: report-carry offered as two alternatives          | RESOLVED   | 009 181–186 commits to single in-error-variant; "or `run` returns alongside" removed                    |
| N3: carry unspecified on cancel/timeout/podman paths  | RESOLVED   | 009 188–200 scopes carry to non-zero-exit; accurate vs `runner.py:307–318`, `builder.py:134,201`        |
| N4: conditional-HOME owner & host-CLI case            | RESOLVED   | 009 119–131, 245–249 gate to `runner build`, exempt host CLI, name 010; matches `entrypoint.sh:19–22`   |
| N5: double timeout (podman `--timeout` + `select!`)   | RESOLVED   | 009 138–165 puts timeout wholly in `podman_run`; no outer `tokio::timeout`; `podman.py:78,115` verified |
| N6: dropped logging rewrite leaves `logging` unspec'd | RESOLVED   | 009 62–64 & 266–272 both clear `config.logging` to `None`                                               |
| Minor: launched image `desc.distro`                   | RESOLVED   | 009 65–69 states `desc.distro`; `runner.py:289`                                                         |
| Minor: `stop(name=None)`→`--all` footgun              | RESOLVED   | 009 162–165 always by name, never `--all`; `podman.py:131–134`                                          |
| Minor: dropped symlink-entrypoint check               | RESOLVED   | 009 238–242 characterizes it moot under binary mount; `runner.py:187–190`                               |
| NEW N7: Testing vs Timeout&cancellation contradiction | UNRESOLVED | 009 291–293 vs 138–165, 255–260                                                                         |
| NEW N8: cancel path `Podman(PodmanError)` unsourced   | UNRESOLVED | 009 76–79, 225–226 vs 152–156                                                                           |
| NEW N9: relocated double timeout race (low)           | UNRESOLVED | 009 / 003; `podman.py:78,115`                                                                           |

## NEW findings ordered by severity

1. **N7 (medium) — Testing section contradicts the Timeout & cancellation
   section.** Testing (009 291–293) asserts elapsing the timeout triggers 009's
   explicit `podman_stop(name = ctr_name)`; the body (138–165, 255–260) says
   timeout is `podman_run`'s job and that explicit stop is cancel-only. Split
   the bullet into a cancel assertion and a timeout assertion.
2. **N8 (medium) — cancel path returns `RunnerError::Podman(PodmanError)` with
   no source.** On cancel the `select!` drops `podman_run` (009 152–156), so no
   `PodmanError` exists to wrap (Errors, 225–226; step 7, 76–79). Add a
   `Cancelled` variant or specify a synthesized error.
3. **N9 (low) — relocated double timeout race-dependent outcome.** `podman_run`
   /003 still sets `--timeout` + await-deadline; the same elapsed timeout can
   surface as `NonZeroExit` xor `Podman`. Note in 003 (which owns the
   mechanism), optionally constrain `--timeout` > await-deadline for
   determinism.
