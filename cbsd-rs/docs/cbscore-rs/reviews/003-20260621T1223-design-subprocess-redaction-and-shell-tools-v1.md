# Review v1 — 003 Subprocess execution, secret redaction & shell-tool wrappers

Adversarial design review of
`cbsd-rs/docs/cbscore-rs/design/003-20260621T1216-subprocess-redaction-and-shell-tools.md`.

Every claim in 003 was verified against the Python source of truth:
`cbscore/src/cbscore/utils/__init__.py`, `utils/git.py`, `utils/podman.py`,
`utils/buildah.py`, `cbscore/src/cbscore/images/skopeo.py` (and
`images/errors.py`, `errors.py`, `builder/signing.py` for cross-checks). The two
`_sanitize_cmd` regex behaviors were confirmed empirically by executing the
Python regex against representative token lists, not by inspection alone.

## Verdict

**Go with changes.** The design is faithful to the Python primitive and the four
wrappers across the dimensions that matter (concurrent per-line streaming,
`out_cb`-empties-captured-strings, env inherit + `extra_env` merge,
non-zero-exit-is-not-an-error, timeout kill-and-reap, the podman cidfile→stop
hook, `git -C`, buildah `run --isolation chroot --`, `--creds` as `Password` in
both buildah and skopeo, worktree random suffix). No finding rises to no-go.
Three corrections are required before the design is used as an implementation
oracle, the most important being a redaction-airtightness gap that the design's
own "can never leak" guarantee does not actually close.

## Confidence score

| Item                                                                             | Points | Description                                                                                                                                                             |
| -------------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Starting score                                                                   | 100    |                                                                                                                                                                         |
| D10: `SecureArg: fmt::Debug` supertrait leaves the "never leak" claim unenforced | -5     | Trait mandates `Debug` on every concrete secret type; a `#[derive(Debug)]` would leak plaintext. Design states the guarantee but specifies no mechanism that forces it. |
| D8: backstop characterization of Python is wrong for `--pass=value`              | -5     | Python's inline regex requires `phrase`; `--pass=secret` is left in plaintext. Design claims Python covers `--pass` inline `=value`.                                    |
| D8: spawn-failure "matching `async_run_cmd`" is inaccurate                       | -5     | Python `async_run_cmd` does not wrap spawn failure; it propagates a raw `OSError`. The Rust `CommandError` is an improvement, not parity.                               |
| **Total**                                                                        | **85** |                                                                                                                                                                         |

Interpretation: 85 → acceptable with noted improvements; fix the three items
before 003 is consumed by the C1 plan.

## Findings (ordered by severity)

### Finding 1 — The "never leak" guarantee is asserted but not enforced (Important)

**Design claim.** Lines 87–108: the redaction contract makes it "structurally
impossible to log the plaintext by formatting the argument"; the trait is
`trait SecureArg: fmt::Debug + Send + Sync`; and `CmdArg`'s `Debug`/`Display`
emit `redacted()` "so `format!("{:?}", arg)` can never leak a secret."

**What the Python code shows.** The Python types hand-implement the censoring in
`__str__`/`__repr__` and expose plaintext only through the `value` property
(`utils/__init__.py:52-63` `Password.__str__` → `<CENSORED>`,
`Password.__repr__` → `Password(<CENSORED>)`, `value` → `self._value`;
`PasswordArg.__str__` line 76; `SecureURL.__str__`/`__repr__` lines 94-100).
Critically, Python's `Password` has **no** auto-generated repr that would dump
`_value` — the censoring repr is written by hand.

**The gap.** 003's guarantee is airtight only for the `CmdArg` enum, whose
`Debug` the design explicitly routes through `redacted()`. But the design also
declares `SecureArg: fmt::Debug` as a supertrait, which requires every concrete
secret type (`Password(String)`, `PasswordArg`, `SecureUrl`) to be `Debug`. The
design never states that those `Debug` impls must be hand-written to emit
`redacted()`, nor that `#[derive(Debug)]` is forbidden. The obvious, idiomatic
implementation — `#[derive(Debug)] struct Password(String)` — leaks the
plaintext through `format!("{:?}", password)` and, transitively, through any
`{:?}` on an `Arc<dyn SecureArg>` (trait-object `Debug` dispatches to the
concrete impl). An implementer following 003 literally can satisfy every stated
requirement and still ship a leak. This is precisely the airtightness the scope
asked to scrutinize, and it is not closed.

**Recommended change.** Make the structural guarantee real in one of two ways,
and state it explicitly: (a) mandate that every concrete `SecureArg` type
hand-implements `Debug` (and `Display`) to emit `redacted()`, and forbid
`#[derive(Debug)]` on any type that holds a secret — with a test that
`format!("{:?}", Password::new("x"))` contains `<CENSORED>` and never `x`; or
(b) drop `fmt::Debug` from the `SecureArg` supertrait so the only reachable
plaintext path is the explicit `plaintext()` method, and let `CmdArg` carry the
sole `Debug` impl (which already routes through `redacted()`). The testing
section (lines 233-236) tests `CmdArg`'s `Debug` but not the concrete types'
`Debug` in isolation; extend it to cover the bare types regardless of which fix
is chosen.

### Finding 2 — Python does NOT redact `--pass=value`; the backstop claim misstates the source (Low)

**Design claim.** Lines 124-127: "Python covers `--pass` / `--passphrase`
(two-token and inline `=value`); the Rust port covers those **and** `--password`
and `-p` — strictly broader than Python."

**What the Python code shows.** `_sanitize_cmd` (`utils/__init__.py:120-143`)
has two mechanisms: a two-token branch that fires only on an exact `--pass` or
`--passphrase` token (lines 130-138), and an inline regex
`r"(.*)(?:(--pass(?:phrase)[\s=]+)[^\s]+)"` (line 121). In that regex
`(?:phrase)` is a non-capturing group with **no** `?` quantifier, so `phrase` is
mandatory — the inline branch matches only `--passphrase<sep>value`, never
`--pass<sep>value`. Executed empirically:

- `['--pass', 'secret']` → `['--pass', '****']` (two-token: redacted)
- `['--passphrase', 'secret']` → `['--passphrase', '****']` (two-token:
  redacted)
- `['--passphrase=secret']` → `['--passphrase=****']` (inline: redacted)
- `['--pass=secret']` → `['--pass=secret']` (**plaintext — not redacted**)

So Python covers `--pass` only in the two-token form, and the inline `=value`
form only for `--passphrase`. The design's parenthetical attributes inline
`=value` coverage to `--pass`, which Python does not do.

**The gap.** The design's _prescription for Rust_ (cover `--pass`,
`--passphrase`, `--password`, `-p`) is sound and a safe strict-broadening, and
byte-parity with Python is an explicit non-goal (001). The defect is only in the
description of Python's behavior, which could mislead an implementer who treats
003 as the test oracle into asserting Python redacts `--pass=value`. Severity is
low because no plain-string credential flag reaches this layer directly: the
only real `--passphrase` user in cbscore embeds it inside a single compound
argument in the GPG-signing path (`builder/signing.py:46-47`,
`"… --passphrase {passphrase}"`), which the inline-space regex catches and which
is owned by the builder subsystem (007), not 003. The backstop is purely
defensive here.

**Recommended change.** Reword lines 124-127 to state Python's actual coverage:
`--passphrase` inline `=value`, and `--pass`/`--passphrase` only as a discrete
preceding token (two-token form); `--pass=value` is left in plaintext by Python.
Keep the Rust prescription as the broader, uniform set, and frame it as fixing
that Python gap rather than merely extending it.

### Finding 3 — Spawn failure is an improvement over Python, not parity with it (Low)

**Design claim.** Lines 78-80: "Spawn failure (binary missing, etc.) is a
`CommandError`," presented under "Behavior, matching `async_run_cmd`" (line 59).

**What the Python code shows.** `async_run_cmd` (`utils/__init__.py:206-267`)
has no try/except around `asyncio.create_subprocess_exec` (lines 225-231); a
missing binary raises a raw `OSError`/`FileNotFoundError` to the caller. Only
the **sync** `run_cmd` wraps `OSError` into a `CESError` (lines 160-162). So the
async primitive the design ports does not map spawn failure to its own error
type.

**The gap.** Mapping spawn failure to `CommandError` is a correct, desirable
improvement, but listing it under "matching `async_run_cmd`" misstates Python's
behavior. An implementer reading 003 as a fidelity spec may believe they are
reproducing existing behavior when they are in fact changing it.

**Recommended change.** Move the spawn-failure sentence out of the "matching"
bullet list, or annotate it as a deliberate divergence (the async Python
primitive lets `OSError` propagate; the Rust port classifies it as
`CommandError`). Optionally note this in Fidelity notes alongside the
timeout/exit-code separation, since both are intentional improvements over the
async Python primitive.

## Notes (not findings)

- **`UnknownRepository` placement is fine.** The design lists
  `UnknownRepository` as a skopeo-module error (lines 199, 209). In Python it is
  actually defined in `cbscore/errors.py:42` (core), and skopeo imports it from
  there (`images/skopeo.py:23`). This is not a defect: 002's rule places
  IO-triggered errors with the subsystem that raises them, and skopeo is the
  sole raiser (`skopeo.py:57`), so colocating it with skopeo is more
  002-consistent than the Python layout. `SkopeoError` and `ImageNotFoundError`
  live in `images/errors.py` (lines 26, 62) with
  `ImageNotFoundError(SkopeoError)` subclassing — consistent with the design's
  `SkopeoError` + `ImageNotFound` framing.

- **Fidelity-notes spot checks all pass.** `git -C <path>` matches
  `git.py:53-55` (uses `path.resolve().as_posix()`); the timeout-vs-exit
  separation correctly resolves the Python FIXME at `utils/__init__.py:258-261`
  (the line reference in 003 at "258" lands inside that FIXME block); the
  worktree random suffix matches `secrets.token_hex(5)` at `git.py:220`; and
  `_reset_python_env` (`utils/__init__.py:174-200`) is correctly identified as a
  PATH-scrub workaround safely dropped.

- **Wrapper interfaces verified.** podman fixed prelude
  (`--security-opt label=disable`, `--cidfile`, `--attach stdout`,
  `--attach stderr`) matches `podman.py:58-69`; the cidfile→`podman_stop(cid)`
  cancellation hook matches `podman.py:118-126`; `podman_stop` time/`--all`
  matches lines 131-134. buildah `_buildah_run` prepend + cid + `--`-divider
  matches `buildah.py:62-70`; `run --isolation chroot` + `with_args_divider`
  matches lines 165-169; `commit --squash` (line 216), `push --digestfile …`
  with `--creds Password(f"{username}:{password}")` only when both are set
  (lines 255-261) matches; `buildah_new_container` = `from <distro>` + initial
  config matches lines 300-336. skopeo `--creds`/`--dest-creds` as `Password`
  matches `skopeo.py:87, 140`; the synchronous `run_cmd` basis (line 43) and the
  intentional sync→async port are correctly characterized; `ImageNotFound` on
  exit 2 / "not found" matches lines 148-154.

- **Git operation set.** The seven operations 003 catalogs match their Python
  signatures; the five Python helpers 003 defers (`get_git_modified_paths`,
  `git_fetch`, `git_pull`, `git_cherry_pick`, `git_get_current_branch`) are
  correctly identified as not-on-core-path. One small precision point for the
  consuming doc (007), not a 003 defect: `git_checkout` (`git.py:226-238`) emits
  `worktree add --track -b <name> --quiet <path> <ref>` — the `--track` flag and
  the branch-name/worktree-name identity (both `ref` with `/`→`--` plus suffix)
  are part of the operation semantics 003 defers to 007, so their absence from
  003 is acceptable under the single-source-of-truth split.
