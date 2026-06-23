# Review — 009 Runner & two-phase execution (v1)

Adversarial design review of
`cbsd-rs/docs/cbscore-rs/design/009-20260622T0439-runner-and-two-phase-execution.md`.

Every claim was checked line-by-line against the Python source of truth:
`cbscore/src/cbscore/runner.py`,
`cbscore/src/cbscore/_tools/cbscore-entrypoint.sh`,
`cbscore/src/cbscore/utils/podman.py`, `cbscore/src/cbscore/cmds/builds.py`.
Settled decisions from 001 (B1/B2, invariant 5, H2/H3) and 003 (the `podman_run`
wrapper) were read for consistency, not relitigated.

## Verdict

**go-with-changes.** The design is structurally sound and faithful on the mount
table, podman flags, in-container argv, config rewrite, and the report
round-trip's read-before-rc / unlink mechanics. But three of its fidelity claims
are wrong in a way that matters:

1. The **cancellation mechanism is described circularly** — 009 punts the
   external-token handling onto 003, which punts it back to 009. The mechanism
   that actually works is never spelled out, and 001 B2 named 009 as the place
   that must spell it out.
2. **`HOME=/runner` is described as "matching the reference's unconditional
   choice"** — but the reference (entrypoint.sh) sets `HOME` _conditionally_.
   The unconditional host set is a real behavioral divergence, not a faithful
   port.
3. **Two "reproduced exactly" claims describe behavior the Python does not
   have** — the partial report is _discarded_, not captured, and the temp
   secrets/config files _leak_ on the success and `PodmanError` paths. 009's
   improvements on both are good, but mislabeled as parity.

None of these blocks the design; all are fixable with wording and one mechanism
paragraph. They must be fixed before this becomes an implementation contract,
because an implementer chasing "reproduced exactly" would either reintroduce a
plaintext-secret leak or fail to thread the report through the error.

## The cancellation mechanism (the headline question)

**Stated mechanism that actually works:** the runner must `select!` on
`token.cancelled()` versus the `podman_run` future; when the token fires (or the
`tokio::time::timeout` elapses), the runner must **explicitly** call
`podman stop <ctr_name>` **by name** before returning. It cannot rely on
"dropping the future," because Rust has no async `Drop`: a dropped future runs
no `.await`, so no `podman stop` can happen on the drop path. The by-name
primitive already exists in Python as `runner.py:343-345` `stop(name=...)` →
`podman_stop(name=...)`; the Rust runner holds `ctr_name` (`runner.py:282`) and
must use it directly.

**Why 009 is incoherent as written.** 009 line 121-128 says the runner "issues
`podman stop` on the container (by the cidfile/name — `podman_run` already does
this on its own timeout, 003)." That parenthetical conflates two distinct
mechanisms:

- The **internal** `podman_run` timeout is a `tokio::time::timeout` _inside_
  `podman_run`; its own `except (CancelledError, TimeoutError)` handler
  (`podman.py:118-126`) reads the cidfile and calls `podman_stop(name=cid)`.
  This fires only when `podman_run`'s _own_ await is cancelled/elapsed. Coherent
  — but it is `podman_run`'s business, not the runner's.
- The **external** `CancellationToken` is held by the runner (the worker
  triggers it, 011). For `podman_run`'s internal handler to fire on that token,
  the token would have to be _threaded into_ `podman_run` so its cancel handler
  runs. 009 does not say that. The alternative — and the one 001 B2 actually
  picked — is that the runner observes the token itself and stops by name.

001 B2 (design 001, lines 116-123) is explicit: "Cancellation uses a
`tokio_util::sync::CancellationToken` ... that **the runner observes** and,
before returning, awaits `podman stop <ctr_name>` ... only the trigger is new;
the container name must be plumbed to the cancellation handler." It then says
"**Detail lives in 009**." So 009 is the document chartered to specify the
observe-and-stop-by-name mechanism — and instead it delegates back to 003. 003
in turn (lines 186-188) says `podman_run`'s on-cancel cidfile→stop "is the hook
the runner's cancellation builds on (009)." The detail has no home; each
document points at the other.

**Recommended change:** 009 must state the mechanism in one paragraph: the
runner `select!`s `podman_run(...)` against `token.cancelled()` (the
`tokio::time::timeout` covers the elapse case); on cancel it calls
`podman_stop(name = ctr_name)` (003) before returning a cancel/timeout
`RunnerError`. Drop the "`podman_run` already does this on its own timeout"
framing for the external-token path — it is true only for `podman_run`'s
internal await, which is a different trigger.

## HOME handling

009 line 114 / line 173-174 says the host sets `HOME=/runner` "matching the
reference's unconditional choice." This is factually wrong about the reference.
entrypoint.sh:19-22 sets it **conditionally**:

```bash
if [[ -z ${HOME} ]] || [[ ${HOME} == "/" ]]; then
  HOME="${RUNNER_PATH}"
  export HOME
fi
```

So Python sets `HOME=/runner` only when the image provides no `HOME` (or `/`).
An image whose `HOME` is, say, `/root` keeps `/root`. Note also that
`runner.py`'s `podman_run` env block (lines 290-294) sets **only** `CBS_DEBUG`,
never `HOME` — `HOME` is set _inside_ the container by the entrypoint, not by
the host. So 009 introduces `HOME` as a **new host-set env** (replacing an
in-container conditional with a host-side unconditional `-e`), and the doc
itself half-admits this ("the entrypoint set this conditionally") in the same
sentence that calls it "unconditional."

**Soundness:** unconditional `-e HOME=/runner` overrides any image-provided
`HOME`. With the uv/venv bootstrap gone, what still reads `HOME` in-container is
gpg (`~/.gnupg`) and buildah/registry auth (`~/.config/containers`,
`~/.docker`). If the builder image deliberately sets `HOME=/root` and seeds
credentials there, the unconditional override silently breaks them; the
conditional form would preserve them.

**Recommended change:** either (a) make the host emit `HOME` **conditionally**
to match Python — only when the image has no usable `HOME` — which is the
faithful port and the safe default; or (b) keep it unconditional but **justify**
it (e.g. "the worker image pins `HOME=/runner`, so the override is a no-op") and
correct the doc to stop claiming the reference is unconditional. Do not leave
the contradiction in the text.

## Partial-report claim contradicts the Python

009 says the report is read before the rc check "so a partial report survives a
build that ... failed the container push" and "the partial report is captured
for telemetry" (lines 138-143), and that this is "reproduced exactly."
Separately it says `RunnerError` "carries the captured stderr" (line 162).

The Python does **not** capture the report on failure. `runner.py:335-338`:

```python
if rc != 0:
    msg = f"error running build (rc={rc}): {stderr}"
    logger.error(msg)
    raise RunnerError(msg)
```

The parsed `report` local (set at 320-325) is read, logged, and unlinked
(332-333) — then **discarded**. On `rc != 0` only `stderr` reaches the error;
the report never reaches the caller. So "captured for telemetry" + "carries
stderr" + "reproduced exactly" cannot all hold: Python reads-logs-unlinks-
discards.

**Recommended change:** decide and state one of:

- **(a) deliberate improvement (preferred):** the non-zero-exit `RunnerError`
  variant carries `Option<BuildArtifactReport>` so the in-process worker (the
  caller that "consumes the returned `Result` + report natively," 009 line
  155-156) can actually use the partial report. This is the right call given the
  whole point of the in-process integration — but it is a _fix_, so drop
  "reproduced exactly" and label it an improvement over Python.
- **(b) parity:** keep Python's read-log-discard and drop "captured for
  telemetry." Then "reproduced exactly" is true but the partial report is
  unused.

## Temp secrets + config files leak in Python — 009's cleanup is a fix, not parity

009's testing section (lines 200-202) claims "the temp components dir, secrets,
and config files are removed on success and on error." Read against `runner.py`,
that is a **fix of a Python bug**, not reproduced behavior:

- `secrets_tmp_path` is created at `runner.py:216` and unlinked **only** on the
  two early error paths: 222 (`ConfigError`/`SecretsError`) and 250-251
  (config-store error).
- `new_config_path` is created at 246 and unlinked **only** at 251.
- The `finally` block at 315-316 cleans **only** the components dir
  (`_cleanup_components_dir`).

So on the **success** path and on the **`PodmanError`** path, both temp files
survive — and `secrets_tmp_path` is **plaintext secrets on disk** (written by
`secrets.store`, 220). This is a real Python leak.

009 doing better (cleaning both on success and on error) is correct and
desirable. But labeling it as the existing contract is dangerous: an implementer
told elsewhere to "reproduce exactly" might match Python and reintroduce the
plaintext-secret leak.

**Recommended change:** add a fidelity note stating that Python leaks the
secrets and config temp files on the success and podman-error paths, that 009
**deliberately fixes** this (RAII/`Drop` guard or explicit cleanup on every
exit), and that the leak must not be reproduced. This is the single most
security-relevant line in the review.

## In-container logging path is rewritten but never mounted

009 reproduces the config rewrite's `/runner/logs/cbs-build.log` logging entry
(line 62) faithfully — but does not note that **`/runner/logs` is never
mounted**. `runner.py:241` sets
`new_config.logging.log_file = /runner/logs/cbs-build.log`, yet `podman_volumes`
(257-266) has no `/runner/logs` entry. The in-container log file is therefore
ephemeral (discarded with the container). Host logging actually comes from the
streamed `output_cb` → `_log_callback` (`runner.py:78-106`, 287) writing the
host `log_file_path`.

This is a Python quirk worth surfacing so the Rust implementer neither (a) adds
a phantom `/runner/logs` mount thinking it is required, nor (b) expects that
in-container file to appear on the host.

**Recommended change:** add a one-line fidelity note: the in-container
`/runner/logs/cbs-build.log` config entry is preserved but intentionally
**unmounted**; host log capture is via the streamed callback, not that file.

## Faithful items (verified, no change needed)

- **Mount table & flags** match `runner.py` exactly: `:Z` on
  `/var/lib/containers` (264 → 009 line 106), `/dev/fuse:rw` device (297),
  `--security-opt label=disable` + `seccomp=unconfined` (`podman.py:60-81`),
  `--network host` (use_host_network, 302), no user namespace
  (`use_user_ns=False`, 300), `--cidfile`, `--name <run-name>` (282/299),
  `--timeout` (301), `--replace` when `replace_run` (304). 009 lines 96-119
  reproduce all of these.
- **In-container argv** is faithful. entrypoint.sh:57-59 emits
  `cbsbuild --config /runner/cbs-build.config.yaml [--debug] runner build $*`
  with `$*` =
  `["--desc", <mount>, "--tls-verify=<bool>", [--skip-build], [--force]]`
  (`runner.py:256,276-280`). 009 lines 87-90 reproduce this, with `--debug`
  correctly replaced by the `CBS_DEBUG` env per H3 — matching
  `cmd_runner_build`'s options (`builds.py:228-259`).
- **Config rewrite** matches `runner.py:229-241`: `secrets`, `vault` (only if
  set), `scratch`, `scratch_containers = /var/lib/containers`, `components`,
  `ccache` (only if set), and the `/runner/logs/cbs-build.log` logging entry
  when a log file is used. 009 lines 56-63 reproduce it. (The unmounted-logs
  quirk above is separate.)
- **Report read-before-rc + unlink** matches `runner.py:318-333` (read into a
  local before the rc check, unlink in `finally`, `None` if absent). 009 lines
  133-143 reproduce the read/unlink/absent mechanics — only the
  capture-on-failure claim (above) overstates it.
- **`CBS_DEBUG=<1|0>`** matches `runner.py:290-294` (derived from
  `logger.getEffectiveLevel() == DEBUG`) and H3.

## Minor notes (low severity)

- **Launched image is `desc.distro`.** `runner.py:289`
  `podman_run(image=desc.distro, ...)`. 009 never states which image is
  launched. Small omission; worth one line.
- **`stop(name=None)` → `podman stop --all`.** `runner.py:343-345` →
  `podman.py:131-138`: a no-name `stop` stops **every** container on the host.
  In the in-process worker this is a footgun. 009's cancellation always has a
  name, but the doc should note whether the Rust `stop` keeps or guards the
  `--all` fallback.
- **Symlink check dropped.** Python rejects a symlink entrypoint
  (`runner.py:187-190`); 009's artifact validation is exists+executable only
  (line 47-50). The mounted artifact is now a trusted image-shipped binary, so
  this is likely fine — but it is a dropped check and should be acknowledged.
- **`gen_run_name` CSPRNG swap.** Python uses `random.choices` (`runner.py:47`,
  PRNG, `# noqa: S311`); 009 (line 147-151) substitutes "a CSPRNG-backed
  equivalent." Harmless and consistent with 003's worktree-suffix precedent, but
  it is a (trivial) non-faithful change, correctly flagged as not
  security-sensitive.

## Confidence score

Design-review adaptation of the confidence-scoring table: D1 = a required
mechanism the design was chartered to specify but did not; D8 = a fidelity claim
that deviates from the Python source of truth; D7 = a security-relevant
mislabeling; D11 = a missing fidelity note for a real quirk.

| Item                                                                | Points | Description                                                                                                                     |
| ------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                      | 100    |                                                                                                                                 |
| D1: cancellation mechanism not specified (circular 009↔003 punt)    | -20    | 001 B2 charters 009 to specify observe-token + stop-by-name; 009 delegates back to 003                                          |
| D8: HOME called "unconditional in the reference"; it is conditional | -10    | entrypoint.sh:19-22 is conditional; unconditional `-e HOME` diverges and can override image `HOME`                              |
| D8: "partial report captured for telemetry / reproduced exactly"    | -10    | `runner.py:335-338` discards the report on `rc != 0`; claims contradict the source and each other                               |
| D7: temp secrets/config cleanup labeled parity, not a fix           | -10    | Python leaks plaintext secrets (216/246 not unlinked on success/PodmanError); "reproduced exactly" risks reintroducing the leak |
| D11: `/runner/logs` rewritten but never mounted — quirk unstated    | -5     | `runner.py:241` vs `podman_volumes` 257-266; in-container log file is ephemeral                                                 |
| D11: launched image `desc.distro` unstated                          | -3     | `runner.py:289`                                                                                                                 |
| D11: `stop(name=None)`→`--all` footgun unstated                     | -3     | `runner.py:344` → `podman.py:134`                                                                                               |
| D8: symlink-entrypoint check dropped, unacknowledged                | -2     | `runner.py:187-190`                                                                                                             |
| **Total**                                                           | **37** |                                                                                                                                 |

Interpretation: 37 lands in "major rework needed" by the raw scale, but the
score is dominated by four wording/spec gaps in an otherwise faithful design,
not by structural unsoundness. The substance (mount table, argv, config rewrite,
report mechanics) is correct. Read the score as: **the design is close, but four
claims must be corrected before it can serve as an implementation contract** —
chiefly the cancellation mechanism (which the implementer cannot derive from 009
as written) and the two "reproduced exactly" mislabelings (which would steer an
implementer into a security leak and a dropped report).

## Findings ordered by severity

1. **Cancellation mechanism is specified circularly (critical).** _Claim:_ 009
   lines 121-128 — runner issues `podman stop` "by the cidfile/name —
   `podman_run` already does this on its own timeout (003)." _Code:_
   `podman.py:118-126` shows `podman_run`'s cidfile→stop fires only on its _own_
   internal await cancel/timeout; the runner's _external_ token is a different
   trigger. 001 B2 (001 lines 116-123) fixes "runner observes the token, stops
   by name" and delegates the detail **to 009**. _Gap:_ 009 sends the detail
   back to 003; no document specifies how the external token reaches a
   `podman stop`. _Change:_ state the `select!`-on-token + explicit
   `podman_stop(name = ctr_name)` mechanism in 009.

2. **Temp secrets/config cleanup mislabeled as parity (high — security).**
   _Claim:_ 009 lines 200-202 — files "removed on success and on error." _Code:_
   `runner.py:216,246` create them; unlinks exist only at 222 and 250-251; the
   `finally` (315-316) cleans only components. Success and `PodmanError` paths
   leak both files, including **plaintext secrets**. _Gap:_ Python is buggy; 009
   silently improves but calls it reproduction. _Change:_ add a fidelity note —
   Python leaks; 009 deliberately fixes; do not reproduce the leak.

3. **HOME described as faithful, but it diverges (high).** _Claim:_ 009 lines
   114/173-174 — `-e HOME=/runner` "matching the reference's unconditional
   choice." _Code:_ entrypoint.sh:19-22 sets `HOME` **conditionally**;
   `runner.py:290-294` host env sets only `CBS_DEBUG`. _Gap:_ unconditional
   override can clobber an image `HOME` (gpg/buildah auth); the doc contradicts
   itself. _Change:_ make it conditional (faithful/safe) or justify
   unconditional and fix the wording.

4. **"Partial report captured for telemetry / reproduced exactly" (high).**
   _Claim:_ 009 lines 138-143, 162. _Code:_ `runner.py:335-338` discards the
   `report` local on `rc != 0`; only `stderr` enters the error. _Gap:_ the three
   statements are mutually inconsistent and none matches Python. _Change:_ pick
   capture-in-error-variant (improvement, relabel) or read-log-discard (parity,
   drop "telemetry").

5. **`/runner/logs` rewritten but unmounted (medium).** _Claim:_ 009 line 62
   reproduces the logging-path rewrite. _Code:_ `runner.py:241` sets it;
   `podman_volumes` 257-266 has no `/runner/logs` mount; host logging is via
   `output_cb`. _Gap:_ unstated quirk; risks a phantom mount or a wrong
   host-file expectation. _Change:_ one-line fidelity note.

6. **Minor (low):** launched image is `desc.distro` (`runner.py:289`) —
   unstated; `stop(name=None)` → `podman stop --all` (`podman.py:134`) footgun —
   unstated; symlink-entrypoint check (`runner.py:187-190`) dropped —
   unacknowledged; `gen_run_name` PRNG→CSPRNG swap — trivial, already flagged.
