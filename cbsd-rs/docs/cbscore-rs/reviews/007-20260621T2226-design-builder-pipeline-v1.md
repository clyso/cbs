# Review — 007 Builder pipeline (v1)

Adversarial design review of
`cbsd-rs/docs/cbscore-rs/design/007-20260621T2216-builder-pipeline.md`. Every
claim was checked line-by-line against the `cbscore` Python sources; nothing is
taken on the author's word. Settled decisions from 001–005 are checked only for
whether 007 honors them, not relitigated.

**Verdict: go-with-changes.** The design is structurally faithful — the
per-stage concurrency model, the S3 layout, the `prepare_builder` sequence, the
script arg order, the patch-ordering summary, and the report-on-both-paths
invariant are all accurate. Two corrections gate the verdict: (F1) the `--force`
description is materially inaccurate about the top-level rebuild path, and (F2)
the upload section omits the silent component-drop when a component has no
release-RPM script. Three smaller items (cwd table, `MissingScriptError`
overstatement, SRPMS key asymmetry) and two port-hygiene notes round out the
findings.

## Answer to the headline question — `--force` semantics

**007 overstates `--force`.** 007 step 3 says: "if a release exists for the arch
and `--force` is not set, reuse it; with `--force`, rebuild." The "with
`--force`, rebuild" half does not match the Python.

Control flow in `builder.py`:

- Line 141 sets `release_desc = await check_release_exists(...)` when
  `storage.s3` is configured.
- Lines 151–162: if a release with an `x86_64` build exists, the code logs
  either "reuse release" (no force) or **"force flag set, rebuild existing
  x86_64 release"** (force) — but in **both** branches it leaves `release_desc`
  populated. `self.force` is read here for the log message only.
- Line 168 `if not release_desc:` is the **sole** gate on `_build_release()`
  (line 170). Because `--force` never clears `release_desc`, a populated
  descriptor means `_build_release` is **never called**.
- The only other use of `self.force` is `_do_build_release` (line 314):
  `if self.storage_config and self.storage_config.s3 and not self.force:` skips
  `check_released_components`, forcing all components to be (re)built. But that
  line is **inside** `_do_build_release`, reached only **through**
  `_build_release`.

Therefore: when `check_release_exists` returns a full release descriptor
carrying an `x86_64` build, `--force` is a **no-op beyond a misleading log
line** — it does not rebuild. The genuine force effect (skip the per-component
reuse check) only ever fires when execution already entered `_build_release`,
i.e. when **no** complete release descriptor existed. 007's "with `--force`,
rebuild" describes a behavior the code does not have on the path that matters (a
complete pre-existing release).

This is a Python defect, directly analogous to the broken `versions list` /
`get_image_desc` cases that 006 chose to **fix, not reproduce** (006 lines 113,
139, 231–238). For consistency with that precedent, the recommendation (see F1)
is that 007 specify the **fixed** behavior (clear `release_desc` when `force` is
set so `_build_release` actually runs), document the Python defect, and not
faithfully port a no-op.

## Confidence score

| Item                                                        | Points | Description                                                                                     |
| ----------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------------- |
| Starting score                                              | 100    |                                                                                                 |
| D8: `--force` "rebuild" mischaracterizes the control flow   | -5     | Step 3 claims a top-level rebuild that `builder.py:151-168` does not perform                    |
| D8: upload section omits the silent no-release-RPM drop     | -5     | `builder.py:576-582` drops the component; 007 lines 166-168 describe only the recorded path     |
| D8: cwd under-specified in the script-contract table        | -5     | deps/build/version all run `cwd=repo`; table annotates only `get_version`                       |
| D8: `MissingScriptError` claimed for all "required" scripts | -5     | Only `get_version.sh` raises it; deps/build raise plain `BuilderError`; release-rpm returns nil |
| D8: SRPMS key asymmetry glossed as "RPMS/SRPMS rel paths"   | -5     | `upload.py:133-134` strips `RPMS/` but keeps `SRPMS/`; 007 line 159 hides this                  |
| D11: `_apply_patches` dead second pass not flagged for port | -5     | `prepare.py:279-292` builds an unused list; port should not copy it                             |
| **Total**                                                   | **70** |                                                                                                 |

Interpretation: 70 sits at the top of the "significant issues — address before
proceeding" band. The deductions are concentrated in documentation-accuracy (D8)
of a design doc rather than implementation defects; once F1 and F2 are corrected
the doc clears comfortably into the acceptable band. No D1/D2/D5/D7 (no deferred
work, duplication, untested path, or security gap) applies to a design artifact
at this stage.

## Findings, ordered by severity

### F1 — `--force` description is inaccurate (gates verdict)

- **Design claim:** 007 step 3 — "with `--force`, rebuild" an existing release.
- **Code:** `builder.py:151-162` reads `self.force` only to choose a log message
  and leaves `release_desc` populated; `builder.py:168` gates `_build_release`
  on `if not release_desc:`; the real force effect is `builder.py:314`
  (`and not self.force`), reachable only inside `_do_build_release` → only when
  no complete release existed.
- **Gap:** On the path that matters (a complete pre-existing `x86_64` release),
  `--force` does nothing but log "rebuild." 007 promises a rebuild that does not
  happen.
- **Recommended change:** Rewrite step 3 to state the Python defect explicitly
  and specify the **fixed** behavior, mirroring 006's treatment of
  `versions list`/`get_image_desc`: when `force` is set, clear/ignore the
  existing `release_desc` so `_build_release` runs and the component-level force
  (`builder.py:314`) takes effect. If the team instead wants bug-for-bug parity,
  say so in as many words and drop the word "rebuild" — but the 006 precedent
  argues for the fix.

### F2 — upload omits the silent no-release-RPM component drop

- **Design claim:** 007 lines 166-168 — the per-component release-RPM location
  is resolved by `get_component_release_rpm` and "recorded in the
  `ReleaseRPMArtifacts` the orchestrator assembles."
- **Code:** `releases/utils.py:34-39` returns `None` (not an error) when the
  `get_release_rpm.sh` script is absent; `builder.py:576-582` then logs "ignore
  component" and `continue`s, so the component is **dropped** from
  `comp_releases` and `comp_rel_versions` — it never reaches the release
  descriptor, even though its RPMs were already built, signed, and uploaded to
  S3.
- **Gap:** 007 describes only the happy branch. A port author would not know the
  descriptor can silently omit an already-uploaded component.
- **Recommended change:** Document the `None`/`continue` branch. Note it as a
  Python behavior and decide reproduce-vs-fix (a missing release-RPM script
  silently amputating a built component from the release is arguably a defect
  worth surfacing as a hard error or at least an explicit report annotation).

### F3 — cwd under-specified in the script-contract table

- **Design claim:** 007's table (lines 48-54) annotates `cwd=repo` only for
  `get_version.sh`; the other rows imply the default cwd.
- **Code:** `install_deps.sh` runs `cwd=repo_path` (`rpmbuild.py:154`);
  `build_rpms.sh` runs `cwd=repo_path` (`rpmbuild.py:101`); `get_version.sh`
  runs `cwd=repo_path` (`utils.py:46`); `get_release_rpm.sh` runs with **no**
  explicit cwd (`releases/utils.py:47`, inherits the process cwd).
- **Gap:** Three of four scripts run with `cwd=repo`; the table singles out only
  one. The task brief explicitly flagged cwd, so this is worth pinning.
- **Recommended change:** Mark `install_deps.sh` and `build_rpms.sh` rows
  `cwd=repo` as well, and state that `get_release_rpm.sh` inherits the caller's
  cwd (no per-script cwd).

### F4 — `MissingScriptError` claimed for all required scripts

- **Design claim:** 007 line 55-56 — "a missing required script is
  `MissingScriptError { script }`."
- **Code:** Only `get_version.sh` raises `MissingScriptError` (`utils.py:39`). A
  missing `install_deps.sh` raises plain `BuilderError` (`rpmbuild.py:135-137`);
  a missing `build_rpms.sh` raises plain `BuilderError` (`rpmbuild.py:227-229`);
  a missing `get_release_rpm.sh` returns `None`, no error
  (`releases/utils.py:34-39`).
- **Gap:** The blanket statement overstates the typed-error surface.
  `MissingScriptError` is the exception for exactly one script.
- **Recommended change:** Scope the claim: `MissingScriptError` is raised only
  for the missing `get_version` script; the other missing scripts surface as
  `BuilderError` (deps/build) or a silent `None` (release-rpm). Tie this to F2.

### F5 — SRPMS S3-key asymmetry glossed over

- **Design claim:** 007 line 159 — keys are
  `…/el<el>.clyso/<RPMS|SRPMS rel paths + repodata>`.
- **Code:** `upload.py:133` computes RPM keys relative to `path_to_rpms` (the
  `RPMS/` dir), so the `RPMS/` segment is **stripped**; `upload.py:134` computes
  SRPM keys relative to `path_to_srpms.parent` (the topdir), so the `SRPMS/`
  segment is **retained**. `_get_repo` repeats the same asymmetric `relative_to`
  on lines 135-136.
- **Gap:** The keys are not symmetric: binary RPMs land at
  `…/el<el>.clyso/<arch>/...` while source RPMs land at
  `…/el<el>.clyso/SRPMS/...`. 007's "RPMS/SRPMS rel paths" reads as if both
  retain (or both strip) their prefix.
- **Recommended change:** State the asymmetry: RPMS are keyed relative to the
  `RPMS/` directory (prefix dropped); SRPMS and their repodata are keyed
  relative to the topdir (the `SRPMS/` prefix is preserved). This is
  layout-parity (invariant 3) and must be reproduced exactly.

### F6 — port hygiene: dead code the port must not copy

- **`prepare.py:279-292`** (`_apply_patches`): after applying patches, the
  function builds a second `patches_lst` (via a `comp_patches_path` walk), sorts
  it, and **never uses it**. Dead code. The port should drop it; 007 should note
  that the patch-application path ends after `git_apply`, not carry the unused
  second pass.
- **`upload.py:91-99`** (`_get_repo._get_repo_r`): the `has_repo` flag is
  initialized `False` and never set `True`, so the `not has_repo` guard is inert
  — `createrepo` is invoked **once per RPM file** in a directory rather than
  once per directory (it produces the same `repodata` each time; benign but
  wasteful and yields duplicate locators). The port should run `createrepo`
  **once per directory containing RPMs**. Worth a one-line note in 007's upload
  section.

## Verified faithful — no change required

These were checked against the code and are accurate; listed so the author knows
they were not overlooked:

- **Stage concurrency.** `_install_deps` is sequential (`rpmbuild.py:131` `for`
  loop); `build_rpms` is parallel (`rpmbuild.py:236` `TaskGroup`); `sign_rpms`
  is parallel across components (`signing.py:121` `TaskGroup`) but sequential
  within a component (`signing.py:91` `for`, with the "can be parallelized" NOTE
  at `signing.py:90`); `s3_upload_rpms` is parallel (`upload.py:160`
  `TaskGroup`). All four match 007 exactly, and the `TaskGroup` →
  abort-on-first-error `JoinSet` (not `join_all`) mapping matches 005's settled
  decision.
- **Script arg order.** `install_deps.sh <repo>` (`rpmbuild.py:145-148`);
  `get_version.sh` no args, `cwd=repo` (`utils.py:41-46`);
  `build_rpms.sh <repo> <el> <topdir> [<version>]` (`rpmbuild.py:84-92` —
  version is conditionally appended); `get_release_rpm.sh <el>`
  (`releases/utils.py:41-44`). All match.
- **S3 base layout.** the key prefix
  `{bucket_loc}/{name}/rpm-{version}/el{el_version}.clyso` is built at
  `upload.py:126` — the `.clyso` suffix and the `rpm-<version>` segment are
  reproduced exactly.
- **`prepare_builder` sequence.** `dnf update` → install `epel-release` →
  `dnf config-manager --enable crb` → `dnf update` → install the toolchain (the
  exact 13-package list at `prepare.py:86-99` matches 007's list) → install
  cosign from the pinned `v2.4.3` GitHub release RPM with
  `rc == 2 && /already installed/` tolerance (`prepare.py:108-123`). Accurate.
- **Patch ordering.** `_get_patch_list` (`prepare.py:129-175`) orders by numeric
  `(\d+)-.*\.patch` prefix within each priority bucket and emits buckets
  **deepest-first** (`reversed(patches_dict.items())`, line 171). The
  version-dir selection (`prepare.py:139-151`) matches the exact name, then
  `get_minor_version` (returns `M.m.p`), then `get_major_version` (returns
  `M.m`) — so 007's labels "minor (`M.m.p`) / major (`M.m`)" correctly mirror
  the Python's (confusingly named) helpers (`versions/utils.py:73-104`).
  Accurate; the golden-test deferral is appropriate.
- **CoreComponent model + kebab aliases.** `core/component.py:31-47` —
  `release-rpm` and `get-version` aliases, `rpm` optional, `containers.path`.
  `load_components` skips a dir with no manifest (warning, line 82) and skips a
  malformed one (error, not fatal, line 89-91). Matches 007 lines 27-46.
- **Report on both paths.** `_write_report` is called on the skipped path
  (`builder.py:134`) and the full-build path (`builder.py:201`). 007's "written
  on both the skipped and full-build paths" is correct — and correctly excludes
  the no-`storage.s3` path, which returns `None` at `builder.py:184` without
  writing a report (a one-line nit at most; 009 covers partial-on-failure).
- **`reset_python_env` drop (settled, 003).** `async_run_cmd` still passes
  `reset_python_env=True` in Python (`rpmbuild.py:101,155`), but
  `_reset_python_env` (`utils/__init__.py:174-200`) is a no-op when `python3`
  resolves under `/usr/bin` (lines 185-187). With no venv on the `cbsbuild` PATH
  the resolution is `/usr/bin/python3`, so the scrub does nothing; 007's
  reasoning is sound and the drop is faithful.
- **GPG signing args + redaction (settled, 003).** `_sign_rpm`
  (`signing.py:34-51`) builds
  `rpm --addsign --define "_gpg_path …" --define "_gpg_name …"` and, with a
  passphrase,
  `--define "_gpg_sign_cmd_extra_args --pinentry-mode loopback --passphrase …"`.
  007's `SecureArg` wrapping of the passphrase-bearing `--define` and the note
  about inherent `ps` exposure are correct.
- **Single-arch / `el<N>` FIXME.** `builder.py:149-151` carries the hardcoded
  `ArchType.x86_64` with the "checking for arch must be done against the version
  descriptor" FIXME; `el_version: int` (`versions/desc.py:51`) yields `el<N>`.
  007 carries this as a known limitation rather than fixing it — consistent with
  the brief.

## Internal consistency with 001–005

No conflicts found. 007's concurrency mapping matches 005 (lines 108-115,
136-138: `JoinSet` abort-on-first-error, never `join_all`). 007's `SecureArg`
usage matches 003 (lines 97-108). 007's `check_release_exists` /
`check_released_components` / `release_desc_upload` /
`release_upload_components` references match 005's signatures (lines 89-99) and
001's C6 parity row. The `ContainerBuilder.build`/`finish` (008), secrets (004),
git/subprocess (003), and host-side report read (009) are referenced, not
specified, as intended.
