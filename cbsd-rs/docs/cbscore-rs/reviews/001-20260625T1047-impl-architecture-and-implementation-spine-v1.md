# 001 — Implementation review: cbscore-rs M0+M1 (v1)

Independent review of the cbscore Rust-port implementation for milestones **M0
(C0 bootstrap/de-risk)** and **M1 (C1 versions create)**. The reviewer distrusts
every implementer claim and verifies against the actual code, the authoritative
designs (001/002/003/006/010/012), the plans (01-bootstrap, 02-versions-create),
and the Python source being ported.

> **Post-review note (history rewrite):** this review covers the commits below
> _before_ findings F1/F3/F4 were addressed; those fixes were subsequently
> folded into the commits they belonged to via `--fixup`/autosquash, so the
> reviewed `6ef1d27`/`f81cd86` are superseded by `de9b022` (2a) and `8ea550e`
> (2b). The original SHAs remain in the reflog and the backup ref; the findings
> and verdict below describe the as-reviewed state.

- **Commits in scope (oldest→newest):** `7b5086c` (C0 scaffold + panic pin),
  `a18c1f9` (C0 musl probe + `ci-cbsd-rs.yaml`), `5d6d890` (C1 pure
  parse/validate), `6ef1d27` (C1/2a versions-create end-to-end), `f81cd86`
  (C1/2b image-descriptor note). Docs-only `2b320d8`/`e0206de` are out of scope
  except where they assert things the code must back.
- **Source in scope:** `cbsd-rs/{cbscore-types,cbscore,cbsbuild}/`,
  `cbsd-rs/ci/musl-probe/`, `.github/workflows/ci-cbsd-rs.yaml`,
  `cbsd-rs/Cargo.toml`.
- **Verification performed:** read all in-scope source and the cited Python
  originals; `cargo test -p cbscore-types -p cbscore -p cbsbuild` (**42 pass**),
  `cargo clippy … --all-targets` (**clean**), `cargo fmt --all --check`
  (**clean**); per-commit boundary inspection via `git show` at each SHA.

## Verdict

**GO** — proceed to M2. The implementation is faithful, well-tested, and the
commit history is clean and bisectable. The findings below are improvements to
fold in, not blockers; none changes wire format, on-disk layout, or the security
posture.

## Scope coverage (plan → code)

Every M0/M1 plan item is implemented; the deferrals are the documented
lands-with-consumer ones, not in-phase gaps.

| Plan item                                                        | State | Evidence                                                                                                                                        |
| ---------------------------------------------------------------- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| Workspace + 3 crates + `panic = "unwind"` pin                    | Done  | `Cargo.toml` members + root `[profile.release] panic = "unwind"`                                                                                |
| Static-musl CI gate (aws-sdk-s3 + vaultrs), PR/push-triggered    | Done  | `ci-cbsd-rs.yaml` `on: push/pull_request`; `file`/`readelf` static check; no `openssl-sys`/`aws-lc-rs`; el9+el10 smoke; shipped-graph guard job |
| `VersionDescriptor` + sub-types, `schema_version` machinery      | Done  | `cbscore-types/src/{version,schema}.rs`; round-trip/marker tests                                                                                |
| `VersionType` enum + `descriptor_path` helper                    | Done  | `version_type.rs`, `store.rs` (helper landed now, reused by M5)                                                                                 |
| Type-layer error taxonomy                                        | Done  | `cbscore-types/src/error.rs`                                                                                                                    |
| `parse_version`/major/minor/normalize/`parse_component_refs`     | Done  | `versions/parse.rs`; 33 parse + 19 normalize Python cases verbatim                                                                              |
| `get_version_type` (name→lookup) + `get_version_type_desc`       | Done  | `versions/version_type.rs`                                                                                                                      |
| `validate_version` (M.m.p or UUIDv7) + `resolve_version`         | Done  | `versions/validate.rs`                                                                                                                          |
| Subprocess primitive (`run_cmd`) + distinct timeout error        | Done  | `utils/subprocess.rs`                                                                                                                           |
| Compiler-enforced redaction (`SecureArg`/`CmdArg`/`Password`)    | Done  | `utils/redact.rs`                                                                                                                               |
| git wrapper (`get_git_user`/`get_git_repo_root`)                 | Done  | `utils/git.rs`                                                                                                                                  |
| Minimal component loader (name+repo)                             | Done  | `components.rs` (needed in M1, not deferred to 007)                                                                                             |
| `version_create_helper`/`create`/title/UUIDv7/`write_descriptor` | Done  | `versions/create.rs`                                                                                                                            |
| Fixed `get_image_desc` + trailing note (skipped for UUIDv7)      | Done  | `images/desc.rs`, `cmds/versions.rs`                                                                                                            |
| clap tree + `CBS_DEBUG=0`-off BOOL parser                        | Done  | `cli.rs`, `bool_parser.rs`                                                                                                                      |

**Documented deferrals (correct, lands-with-consumer):** async `out_cb` (runner
streaming, M2), `PasswordArg`/`SecureUrl` (C3/C4/C5), the full `CoreComponent`
model (007/C3), `build`/`runner build`/`versions list` commands (M2/M3). These
are future-milestone scope, not omissions of this phase — no D1.

## What was verified correct

- **Regex/grammar parity.** The version grammar and `COMPONENT@REF` regexes are
  faithful to `utils.py:44-59,148`; the 33 parse + 19 normalize parametric cases
  are ported **verbatim** and pass. A UUID string correctly fails
  `parse_version` (so `validate_version` falls through to the v7 check), and a
  v4 UUID is rejected (only `SortRand`/v7 accepted).
- **Title parity (`_do_version_title`).** Prefix uppercasing, the
  `version M.m.p` body, and the `-`-split / first-`.`-segment-uppercase suffix
  rule all match Python (`ces-v20.2.1-rc.1` → `… CES version 20.2.1 (RC 1)`;
  `20.2.1-ga.1-hotfix` → `… (GA 1, HOTFIX)`). The new UUIDv7 title derives the
  ISO-8601 UTC timestamp from the **UUID-embedded** time (not wall-clock), so a
  user-supplied v7 yields its own creation time — matches design 006.
- **`get_image_desc` is fixed, not reproduced.** The returned descriptor is the
  **matched** one (`found.map(|(_, d)| d)`), fixing Python's last-parsed-wins
  bug; the `v`-optional `M.m.p` extraction drives only the file pre-filter while
  the authoritative match is exact raw-form membership in `releases`; malformed
  candidates are **skipped** (not re-raised); two matches conflict; missing
  `desc/` or no-`M.m.p` is `NoSuchVersion` (skipped by the caller for v7 /
  patch-less). Five focused tests cover each branch.
- **Redaction is compiler-enforced.** `SecureArg` has no `Debug`/`Display`
  supertrait; `Password` derives neither; `CmdArg`'s hand-written
  `Debug`/`Display` emit `redacted()`; the primitive logs via `sanitize_cmdline`
  (handles two-token `--password v` and inline `--pass=v`, broadened to
  `--password`/`-p` per design 003). `format!("{:?}", secret)` cannot leak —
  pinned by test.
- **Subprocess timeout/kill + deadlock-safety.** stdout/stderr are read
  concurrently with `wait()` via `tokio::join!` (no pipe-buffer deadlock); on
  elapse the timeout future is bound to `timed` first (ending `collect`'s borrow
  of `child`) before `start_kill()`+`wait()` reuse `child` — the borrow dance is
  correct and commented; timeout is a **distinct** `CommandError` variant
  (resolves the Python FIXME). Non-zero exit returns `CmdOutput`; spawn failure
  is typed. All four behaviors are tested.
- **CLI surface (design 010).** `--config` is a non-global root arg (frees `-c`
  for `--component`), defaulted, not validated at parse; `versions create` never
  loads it; `VERSION` is optional (UUIDv7 divergence); `CBS_DEBUG=0` is **off**
  via the shared Click-equivalent BOOL parser (not a presence flag) — pinned by
  test. `Cli::command().debug_assert()` guards the tree.
- **C0 musl gate is real and correctly placed.** `ci-cbsd-rs.yaml` triggers on
  `push`/`pull_request` (not tags), builds the probe static in `alpine:3.21`
  `--locked`, asserts no `INTERP` (readelf) + `file` static + no `openssl-sys`
  - no `aws-lc-rs` (rustls+ring), smoke-runs on el9+el10, and a second job
    guards that the shipped graph excludes `aws-sdk-s3`/`vaultrs`. The probe is
    `[workspace] exclude`d with a committed `Cargo.lock`; `panic = "unwind"` is
    pinned at the workspace root (invariant 6).

## Commit-boundary assessment (git-commits smell test)

| Commit  | One-sentence purpose                    | Parent compiles | Revertable | Testable                              | No dead code                                          | Verdict                                         |
| ------- | --------------------------------------- | --------------- | ---------- | ------------------------------------- | ----------------------------------------------------- | ----------------------------------------------- |
| 7b5086c | scaffold 3 crates + pin panic           | n/a (first)     | yes        | `--version`                           | yes                                                   | Pass (small, but a legitimate de-risk scaffold) |
| a18c1f9 | prove heavy deps link static-musl in CI | yes             | yes        | CI gate runs on PR                    | yes (probe excluded)                                  | Pass                                            |
| 5d6d890 | pure version types + parse/validate     | yes             | yes        | round-trip/golden tests               | yes (`mod.rs` exposes only what exists)               | Pass                                            |
| 6ef1d27 | versions create end-to-end (2a)         | yes             | yes        | `versions create` writes a descriptor | yes (no `images` mod; `serde_json` not yet needed)    | Pass (oversized — see note)                     |
| f81cd86 | fixed `get_image_desc` + note (2b)      | yes             | yes        | image-desc resolve/conflict/skip      | yes (`serde_json` lands with its first consumer here) | Pass                                            |

- **No layer-by-layer split, no dead-code commit.** Verified by inspecting
  `lib.rs`/`versions/mod.rs`/`Cargo.toml` at each SHA: each lower layer
  (subprocess→redact→git→create→CLI) lands **with** its consumer in 2a, and
  `serde_json` enters `cbscore`'s manifest only in 2b where `images/desc.rs`
  first uses it. No commit needed `#[allow(dead_code)]`/`#[allow(unused)]`.
- **2a sizing (~1540–1600 lines incl. tests).** Over the ~800 authored
  guideline, but the subprocess+redaction+git+create+CLI chain is the
  tightly-coupled unit the plan's S-5 note predicted: every lower layer's only
  non-test consumer is the next layer up, all in 2a, so any further split would
  ship dead code. The one clean seam (the non-fatal image-desc note) **was**
  carved into 2b. The size is justified by coupling, not negligence — an
  observation, not a violation.
- **Messages** are intent-first (Ceph style), DCO-signed, with exactly one
  `Co-authored-by` trailer matching the active model.

## Findings (by severity)

### F1 (medium) — CLI handler orchestration is untested (D5)

`cbsbuild/src/cmds/versions.rs::create` is the one piece of M1 business logic
with **no** test. Every library function it calls is tested, but the handler's
own logic is not: the operation **ordering** (type → resolve → git-user → refs →
helper → write → note), the per-stage error routing to `ExitCode`, and the
`has_mmp` decision that gates the trailing `get_image_desc` note
(`parse_version(...).map(|p| p.minor.is_some() && p.patch.is_some())`). The skip
behavior is covered indirectly by `images::desc`'s
`version_without_mmp_has_nothing_to_key_on`, but the **handler's** branch is
not. Risk is low (thin glue), but the `has_mmp` gate is genuine business logic
that could regress silently. Recommend a small integration test (real temp git
repo + `components/`) asserting a UUIDv7 create skips the note and an `M.m.p`
create emits/omits it correctly.

### F2 (low) — component loader is more lenient than Python (D8)

`ComponentDef` requires only `name`+`repo`, so a `cbs.component.yaml` missing
the `build`/`containers` sections **loads successfully** in the port, whereas
Python's `CoreComponent` (`core/component.py:42-47`, both sections required)
would reject it and warn+skip. For `versions create` this is harmless (and the
as-built note flags it as intentional minimalism), but it is a real parity
divergence: a structurally-broken component definition that Python skips, the
port silently accepts. When the full `CoreComponent` model lands at 007/C3,
ensure the stricter validation is reintroduced there so the two readers do not
disagree about what is a "valid" component.

### F3 (low) — 11–12-positional-argument functions (D4)

`create` and `version_create_helper` carry 11–12 positional parameters with
`#[allow(clippy::too_many_arguments)]`. Justified as a faithful port of Python's
signatures, but non-idiomatic Rust and error-prone at call sites (positional
`&str` soup — e.g. `"r", "n", None, "u", "e"` in tests). A `CreateInputs` params
struct would remove the `allow`, make call sites self-documenting, and cost
nothing behaviorally. Optional; revisit if these signatures grow in M2.

### F4 (informational) — watch items for later phases (no deduction)

- **C4a static re-assert.** The C0 probe proves only its **own** graph links
  static; when `aws-sdk-s3`/`vaultrs` become real `cbscore` deps at C4a, CI must
  extend the static assertion to the shipped `cbsbuild` (the plan's forward note
  already flags this). A feature slip there would pass the probe yet ship a
  non-static binary.
- **`serde-saphyr 0.0.26`** is a `0.0.x` (pre-1.0) YAML dependency on the
  config/component read path; pin and watch for churn as M2 leans on it harder.
- **`git config user.name`** (no `--get`): matches Python (`git.py:77`) and is
  fine for a single-valued key, but a multi-valued key would print multiple
  lines; acceptable parity, noted only for awareness.
- **Test cwd mutation.** `utils::git` and `versions::create` tests call
  `std::env::set_current_dir`, a process-global; harmless at current scale (no
  observed flake) but a known parallel-test hazard if more cwd-sensitive tests
  are added.

## Confidence score

| Item                                         | Points | Description                                                                                       |
| -------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------- |
| Starting score                               | 100    |                                                                                                   |
| F1 / D5: untested CLI handler orchestration  | -15    | `cmds::versions::create` ordering, error routing, and the `has_mmp` note-gate have no direct test |
| F2 / D8: component-loader leniency vs Python | -5     | Port accepts a `cbs.component.yaml` lacking `build`/`containers` that Python rejects              |
| F3 / D4: 11–12-positional-arg functions      | -5     | `#[allow(clippy::too_many_arguments)]` ×2; idiomatic Rust would use a params struct               |
| **Total**                                    | **75** |                                                                                                   |

**Interpretation:** 75 — top of the "acceptable with noted improvements" band.
The single material finding (F1) is a thin-glue test gap; F2/F3 are small. Wire
format, on-disk layout, redaction, the timeout/kill primitive, the
`get_image_desc` fix, the musl gate, and commit hygiene are all correct and
tested. Fixing F1 before M2 (and tracking F2 into 007/C3) is the recommended
path; none blocks the go.

## Recommended actions before / during M2

1. **Add an integration test for `cmds::versions::create`** (F1) — real temp git
   repo + `components/`, asserting the UUIDv7-skips-note and
   `M.m.p`-emits/omits-note branches and the error exit codes.
2. **Track F2** as an explicit acceptance criterion for 007/C3: the full
   `CoreComponent` model must restore Python's `build`/`containers`-required
   validation so both component readers agree.
3. **Consider F3** (params struct) opportunistically when `create`'s signature
   is next touched in M2.
4. **Carry F4's C4a static re-assert** into the M2/C4a plan's acceptance gate.
