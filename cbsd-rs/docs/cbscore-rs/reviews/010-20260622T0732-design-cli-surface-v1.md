# Review — 010 CLI surface (`cbsbuild`) — v1

- **Type:** design review (adversarial, design-level)
- **Target:** `cbsd-rs/docs/cbscore-rs/design/010-20260622T0725-cli-surface.md`
- **Round:** v1 (first review of this document)
- **Reviewer mandate:** distrust the implementer; verify every flag-table row
  line-by-line against the Click definitions in the Python source. This is the
  document that resolves 000's M1, so a single missing or mis-shaped flag is a
  material finding.
- **Verdict:** **GO** — the parity table that M1 demanded is accurate. Two minor
  rigor gaps (a bool-lexeme spec gap and a one-clause overstatement) are worth
  fixing but do not block.
- **Confidence:** 90 / 100.

## Method

Every flag row was checked against the actual `@click.option` /
`@click.argument` declarations in the Python source — not against assumptions
about Click. The files read in full:

- `cbscore/src/cbscore/__main__.py` (root group)
- `cbscore/src/cbscore/cmds/__init__.py` (`Ctx`, `with_config`, `pass_ctx`)
- `cbscore/src/cbscore/cmds/builds.py` (`build` + hidden `runner build`)
- `cbscore/src/cbscore/cmds/versions.py` (`versions create` / `list`)
- `cbscore/src/cbscore/cmds/config.py` (the dropped `config` group)
- `cbscore/src/cbscore/cmds/advanced.py` (the empty hidden `advanced` group)
- `cbscore/src/cbscore/_tools/cbscore-entrypoint.sh` (`CBS_DEBUG`/`HOME`)
- `cbscore/src/cbscore/runner.py` (the dead-secrets-write claim)

Cross-referenced designs read to confirm the references resolve and do not
contradict: 009 (runner / two-phase), 006 (versions / UUIDv7 / C7), 004
(config/secrets/vault), 001 (architecture spine), and 000 (the M1 wording).

## Per-claim verification

### Claim 1 — the `build` flag table (`builds.py:36-131`)

Verified row-by-row against the Click decorators. **All ten rows are correct.**

| Row                   | Source                                                      | Doc claim                                    | Verdict           |
| --------------------- | ----------------------------------------------------------- | -------------------------------------------- | ----------------- |
| `DESCRIPTOR`          | `@click.argument` `exists=True`, `required=True` (`:37-45`) | arg, required (existing), kept               | OK                |
| `--cbscore-path`      | `required=True` (`:46-59`)                                  | value, **required**, dropped                 | OK                |
| `-e/--cbs-entrypoint` | `required=False` (`:60-74`)                                 | value, optional, dropped                     | OK                |
| `--timeout`           | `type=float`, `default=4*3600.0` (`:75-82`)                 | value, `default=14400.0`, kept               | OK (4×3600=14400) |
| `--sign-with-gpg-id`  | `required=False` (`:83-90`)                                 | value, optional, kept                        | OK                |
| `--sign-with-transit` | `required=False` (`:91-98`)                                 | value, optional, kept                        | OK                |
| `--log-file`          | `required=False` (`:99-114`)                                | value, optional, kept                        | OK                |
| `--skip-build`        | `is_flag=True`, `default=False` (`:115-120`)                | flag, `default=false`, kept                  | OK                |
| `--force`             | `is_flag=True`, `default=False` (`:121-126`)                | flag, `default=false`, kept                  | OK                |
| `--tls-verify`        | `default=True`, no `is_flag`, no `type` (`:127-131`)        | value, `default=true`, **value-taking BOOL** | OK                |

The `--tls-verify` shape is the easy-to-flatten one, and the doc gets it right:
Click infers `BOOL` from the `default=True` with no `is_flag` and no `type`, so
it is a value-taking option (`--tls-verify=<bool>`), **not** a bare flag. The
doc's warning that a bare-flag modelling would reject `--tls-verify=false` is
correct. (Caveat on the accepted lexeme set — see Finding 1.)

### Claim 2 — the `runner build` table (`builds.py:228-259`)

Verified. **All four rows correct.** `--desc` is `required=True` (`:235-242`);
`--skip-build` / `--force` are `is_flag=True, default=False` (`:243-254`);
`--tls-verify` is the same inferred value-taking BOOL as on `build`
(`:255-259`). The group `cmd_runner_grp` is `hidden=True` (`:223`); the port
keeps it hidden — correct. The in-container contract the doc states
(`--config … runner build --desc … --tls-verify=… [--skip-build] [--force]`)
matches this parser and agrees with 009's emit (009:99-102, 009:296-301).

One subtlety the doc handles correctly but does not spell out: in **Python**,
the host emits `f"--tls-verify={tls_verify}"` with a Python `bool`
(`runner.py:256`), which renders capitalized (`--tls-verify=True/False`);
Click's inferred BOOL accepts that case-insensitively. In the **Rust** port that
whole emit path is replaced by 009's `runner::run` (both ends the same Rust
binary), so there is no cross-language capitalization hazard — the framing in
the doc (host emit and in-container parse "agree") is the right one. See Finding
1 for the residual lexeme-set point.

### Claim 3 — the `versions create` table (`versions.py:179-265`)

Verified. **All ten rows correct.**

| Row                           | Source                                                            | Doc claim                                        | Verdict |
| ----------------------------- | ----------------------------------------------------------------- | ------------------------------------------------ | ------- |
| `VERSION`                     | `@click.argument("version", type=str)` (`:180`) — Python required | arg, required → **optional** (UUIDv7 divergence) | OK      |
| `-t/--type`                   | `default="dev"` (`:181-191`)                                      | value, `default=dev`, kept                       | OK      |
| `-c/--component`              | `multiple=True`, `required=True` (`:192-201`)                     | value×N, **required**, kept                      | OK      |
| `--components-path`           | `multiple=True`, `required=False` (`:202-216`)                    | value×N, optional, kept                          | OK      |
| `-o/--override-component-uri` | `multiple=True`, `required=False` (`:217-226`)                    | value×N, optional, kept                          | OK      |
| `--distro`                    | `default="rockylinux:9"` (`:227-234`)                             | value, `default=rockylinux:9`                    | OK      |
| `--el-version`                | `type=int`, `default=9` (`:235-242`)                              | value, `default=9` (int)                         | OK      |
| `--registry`                  | `default="harbor.clyso.com"` (`:243-250`)                         | value, `default=harbor.clyso.com`                | OK      |
| `--image-name`                | `default="ces/ceph/ceph"` (`:251-258`)                            | value, `default=ces/ceph/ceph`                   | OK      |
| `--image-tag`                 | `required=False` (`:259-265`)                                     | value, optional, kept                            | OK      |

The `VERSION`-optional row is correctly labelled a **deliberate divergence**
(not a parity bug); the cross-ref to 006's UUIDv7 path resolves (006:88-111).
And `cmd_versions_create` (`:266-291`) takes **no `Ctx`/config** — it is a plain
function (not decorated with `@pass_ctx` or `@with_config`), calling
`asyncio.run(version_create(...))`. The doc's claim that "`versions create`
never loads the config" is correct and is the one command that runs with no
config file present.

### Claim 4 — the `versions list` table (`versions.py:294-332`)

Verified. **Both rows correct.** `-v/--verbose` is `is_flag=True, default=False`
(`:295-302`); `--from` (dest `s3_address_url`) is `required=True` (`:303-310`).
`cmd_versions_list` is decorated `@with_config` (`:311`), so it loads the config
— matching the doc. The cross-ref to 006's C7 fix (the broken two-arg
`list_releases` call and the no-S3-config error) resolves (006:113-124). The
doc's note that output is approximate human-readable text (no machine consumer,
no byte-parity) is reasonable and consistent with the Python `click.echo`
listing (`versions.py:134-152`).

> **Note (trivial).** The `versions list` table heading cites
> `versions.py:294-310`, but the handler body runs to `:332`. The cited range
> covers exactly the option decorators the table describes, so the citation is
> fine for the table's purpose; flagged only for completeness, no action needed.

### Claim 5 — `CBS_DEBUG` semantics

The core decision — **`CBS_DEBUG=0` → OFF**, truthy-only (`1`/`true`/`yes`,
case-insensitive), with an explicit warning against a presence-based clap env
binding (`ArgAction::SetTrue` + `.env(...)` would treat the non-empty string
`"0"` as set) — is **correct and important**. 009 explicitly forwards
`-e CBS_DEBUG=<1|0>` (009:122, 259), so a presence check would silently invert
the off case. The doc's prescription (value parser or explicit `env::var` read,
plus a pinned `CBS_DEBUG=0 → off` test) is the right fix.

Two precision points (see Finding 2): the claim that the truthy set "matches …
the entrypoint's `CBS_DEBUG == "1"` test" is a slight overstatement — the
entrypoint accepts **only** `"1"` (`entrypoint.sh:54-55`:
`[[ -n ${CBS_DEBUG} ]] && [[ ${CBS_DEBUG} == "1" ]]`), narrower than
`1/true/yes`. The "matches Click's `BOOL` env parsing" half is accurate
(`__main__.py:39` is `is_flag=True, envvar="CBS_DEBUG"`, and Click coerces the
env var through BOOL).

### Claim 6 — the `HOME` hook

Verified against `entrypoint.sh:19-22`:

```sh
if [[ -z ${HOME} ]] || [[ ${HOME} == "/" ]]; then
  HOME="${RUNNER_PATH}"   # RUNNER_PATH=/runner
  export HOME
fi
```

The doc's statement — set `HOME=/runner` iff `HOME` is unset/empty **or**
exactly `/`, otherwise leave it (so an image's `HOME=/root` survives) — exactly
matches the shell condition (`-z` covers both unset and empty). Scoping it to
the **first action of the `runner build` handler only** (the sole
PID-1-in-builder path) is correct, and it resolves 009's explicit
forward-delegation: 009 says "010 (CLI surface) owns wiring this startup hook
onto the `runner build` subcommand" (009:132-133, 261-265). The doc's insistence
that `build`, `versions …`, and the root group do **not** touch `HOME` is
consistent with 009's reasoning that a host-set `-e HOME=/runner` would wrongly
override an image's `/root`. No discrepancy.

### Claim 7 — the dead plaintext-secrets write

Verified in `builds.py` and `runner.py`. `cmd_build` does:

1. `Config.load` (`:148-152`);
2. fold `--sign-with-*` into `config.signing`, constructing `SigningConfig` if
   absent (`:154-162`);
3. pre-check `--log-file` exists (`:164-171`);
4. **marshal secrets to a temp file** — `config.get_secrets()` then
   `secrets.store(secrets_path)` into a `tempfile.mkstemp(...".secrets.yaml")`
   (`:173-186`);
5. call `runner(...)` (`:188-203`).

The `runner(...)` call at `:191-203` passes `desc_path, cbscore_path, config`
plus `entrypoint_path/timeout/log_file_path/skip_build/force/tls_verify` — it
**never** passes `secrets_path`. The `secrets_path` file is only `unlink`ed in
the `finally` (`:219-220`); it is never read, never mounted. Independently,
`runner()` marshals its **own** secrets temp file (`runner.py:214-222`:
`mkstemp(...secrets.yaml)`, `config.get_secrets()`, `secrets.store(...)`) and
mounts **that** at `/runner/cbs-build.secrets.yaml` (`runner.py:262`). So the
`cmd_build` temp file is a genuine dead write of plaintext credentials to disk
for zero effect — the doc's "broken Python the port fixes" claim is
**accurate**. The port's `build` doing no secrets marshalling (all secret
handling + RAII cleanup in `runner::run`, 009) is the right resolution, and it
is consistent with 009's own temp-file-cleanup security fix (009:277-281). Note
this is a distinct issue from 009's leak finding (009 fixes the runner's own
`finally` leaking the secrets file on success/PodmanError paths); 010 fixes the
redundant `cmd_build` write. Both are real and non-overlapping.

### Claim 8 — the dropped surface, reconciled against 000's M1

**Config group.** `config.py` defines `cmd_config` (`:312-314`) with two
subcommands: `init` (`:317-456`) and `init-vault` (`:459-478`). The doc
enumerates `init`'s flags as dropped: `--components`, `--scratch`,
`--containers-scratch`, `--ccache`, `--vault`, `--secrets`,
`--for-systemd-install`, `--systemd-deployment`, `--for-containerized-run`
(verified against `:318-416`), plus `init-vault`'s `--vault` (`:460-471`). **The
enumeration matches the source exactly.**

**Advanced group.** `advanced.py:22-24` is `@click.group(hidden=True)` with a
`pass`ing body and no registered subcommands — the doc's "empty hidden group,
nothing to port" is correct.

**The M1 reconciliation is the load-bearing check, and it holds.** 000's M1
resolution names only `--cbscore-path` and `-e/--cbs-entrypoint` as intentional
**flag**-level drops, with "every other flag is kept" (000:242-261, 325). 010
additionally drops two whole **command groups** on top of that. The doc is
honest about this: it frames the command-level drops as "later scope decisions
that **supersede** M1's flag enumeration," and grounds them in 001. That
grounding is real:

- 001 non-goals: "**No `config init` command**" (001:31-32); §"Configuration is
  hand-authored": "The Rust port drops the entire `config` command group"
  (001:159-173); correctness invariant 2: "the entire `config` group is
  intentionally dropped (configuration is hand-authored)" (001:281); the
  subsystem index explicitly carries `cmds/` "(minus `config`)" for 010
  (001:266, 269-271).
- 004 confirms the formats are specified for hand-authoring and that the README
  is the operator-facing rendering (004:16-19, 241-248).

So the command-level drops are a settled architectural decision, not a parity
gap that contradicts M1 — the doc correctly distinguishes "M1 enumerated the
flag drops" from "001 decided the command drops." No finding.

## Cross-reference integrity

- **009 ↔ 010** — the in-container argv (009:99-102) and the `runner build`
  parser (010 table) agree; the `HOME` delegation (009:132-133) is picked up by
  010; the dead-secrets framing in 010 is consistent with (and distinct from)
  009's runner-temp-file leak fix. No contradiction.
- **006 ↔ 010** — UUIDv7-optional `VERSION` (006:88-111) and the C7
  `versions list` fix (006:113-124) both resolve as cited.
- **004 ↔ 010** — config/secrets/vault formats documented for hand-authoring,
  matching 010's "documented in the README" claim.
- **001 ↔ 010** — binary-mount (B1), `config`-group drop, and the CLI-parity
  invariant all line up.
- **011** is referenced (worker consumes the report natively) and does not yet
  exist; the references to it are forward-looking by design, not verifiable
  here, and are not treated as defects.

## Findings (by severity)

### Finding 1 (minor) — `--tls-verify` accepted-lexeme set is unspecified

The doc rigorously pins `CBS_DEBUG`'s accepted lexemes (`1/true/yes`,
case-insensitive) but says **nothing** about the lexeme set `--tls-verify`
accepts. This matters because Python's Click infers a **BOOL** parameter, whose
converter accepts a broad case-insensitive set: `1/0`, `true/false`, `t/f`,
`yes/no`, `y/n`, `on/off`. clap's **default** `bool` value parser accepts only
`true`/`false`. An operator who runs `build --tls-verify=yes` or
`--tls-verify=on` (valid under Python) would hit a parse error unless the
implementation opts into clap's `BoolishValueParser` (or a custom parser
mirroring Click's BOOL set). The in-container emit from 009's Rust `runner::run`
is fine either way (it controls its own formatting), so this is host-CLI
operator-parity only — hence minor — but the doc should pin the set the same way
it pinned `CBS_DEBUG`'s, so the implementer does not silently inherit clap's
narrower default. Same applies symmetrically: confirm `--skip-build`/`--force`
remain bare flags (no value) — the doc states this, and it is correct.

### Finding 2 (minor) — "matches the entrypoint's `CBS_DEBUG == "1"` test" overstates

In the `CBS_DEBUG` section the doc says its truthy set (`1/true/yes`) "matches
both Click's env-`BOOL` coercion on the host **and** the entrypoint's
`CBS_DEBUG == "1"` test in the container." The Click half is accurate; the
entrypoint half is not exact — `entrypoint.sh:54-55` accepts **only** the
literal `"1"`, narrower than `1/true/yes`. This is harmless in practice because
the only producer that ever feeds the in-container binary is 009, which emits
strictly `1|0` — but the word "matches" is a slight overstatement. Recommend
softening to "is a superset of the entrypoint's `1`-only test; the only emitter
(009) uses `1|0`, which both accept." (The entrypoint is being replaced anyway,
so this is documentation precision, not a behavioral risk.)

### Note (trivial, no action) — `versions list` line citation

The `versions list` table cites `versions.py:294-310`; the handler runs to
`:332`. The cited range is exactly the option decorators the table documents, so
it is correct for the table. Flagged only for completeness.

## Confidence score

| Item                                                               | Points | Description                                                                         |
| ------------------------------------------------------------------ | ------ | ----------------------------------------------------------------------------------- |
| Starting score                                                     | 100    |                                                                                     |
| D8: `--tls-verify` accepted-lexeme set unspecified (Finding 1)     | −5     | Spec gap vs Click BOOL; clap default bool parser is narrower (host operator-parity) |
| D11: "matches the entrypoint `== "1"` test" overstated (Finding 2) | −5     | Documentation imprecision in the `CBS_DEBUG` section; no behavioral risk            |
| **Total**                                                          | **90** | Ready to merge; address the two minor notes                                         |

**Interpretation:** 90/100 — ready to merge with minor noted improvements. The
single most important job — the per-subcommand flag parity table that 000's M1
demanded — is **accurate in every row across all four tables** (`build` ×10,
`runner build` ×4, `versions create` ×10, `versions list` ×2). The hard-to-get
shapes (the value-taking BOOL `--tls-verify`, the three repeatable `value×N`
options, the required-repeatable `-c`, the no-config `versions create`, the
optional `--image-tag`) are all correct. The dead-secrets-write claim, the
`HOME` condition, the `CBS_DEBUG`-truthy-not-presence decision, and the
dropped-surface/M1 reconciliation all hold against the source.

## Required actions before proceeding

None blocking. Recommended (both minor):

1. Pin `--tls-verify`'s accepted lexeme set in the doc (mirror the `CBS_DEBUG`
   treatment); the implementation must opt into a Click-equivalent BOOL parser,
   not clap's narrower `true/false`-only default (Finding 1).
2. Soften the "matches the entrypoint's `CBS_DEBUG == "1"` test" clause to
   "superset of the `1`-only test; the only emitter (009) uses `1|0`" (Finding
   2).
