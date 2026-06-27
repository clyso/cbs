---
seq: "007"
type: design
title: builder-pipeline
version: 2
updated: 2026-06-27T20:15
---

# 007 — Builder pipeline (v2)

> **Version 2** — supersedes v1 (`007-20260621T2216-builder-pipeline.md`). v2
> adds a single fidelity note (patch traversal tolerates stray files), recording
> an as-built divergence surfaced by the C3 implementation review; the rest of
> the design is unchanged from v1.

This is the reference design for the builder pipeline of the `cbscore` library:
the in-container build orchestrator (`Builder.run`), its four stages (prepare →
rpmbuild → signing → upload), the `CoreComponent` model (`cbs.component.yaml`)
and the component-script contract, and the build report write. It owns these as
the single source of truth. Read 002 for `VersionDescriptor` / `ReleaseDesc` /
`BuildArtifactReport`, 003 for the subprocess primitive + git wrapper, 004 for
`SecretsMgr`, 005 for the S3 release operations, and 001 for the two-phase model
and the commit map.

Source of truth: the `builder/` stage modules — `builder.py`, `prepare.py`,
`rpmbuild.py`, `signing.py`, `upload.py`, `utils.py` — plus
`cbscore/core/component.py` and `cbscore/releases/utils.py`.

This pipeline runs **inside the builder container** (the host runner that spawns
the container is 009). The container-image assembly it invokes
(`ContainerBuilder.build`/`finish`) is owned by 008 and only referenced here at
its call site.

## Component model & script contract

Source: `core/component.py`. `cbs.component.yaml` is a **read input** authored
in the deployment's `components/<name>/` directory (not produced by cbscore), so
it is **unversioned** (no `schema_version`, per 001).

```rust
struct CoreComponent {
    name: String,
    repo: String,
    build: CoreComponentBuild,
    containers: CoreComponentContainers,    // { path: Utf8PathBuf }
}
struct CoreComponentBuild {
    rpm: Option<CoreComponentBuildRpm>,     // { build, release_rpm "release-rpm" }
    get_version: String,                    // "get-version"
    deps: String,
}
```

`load_components(paths)` scans each path's immediate subdirectories for a
`cbs.component.yaml`, loads it, and returns
`map<name, CoreComponentLoc { path, comp }>`; a subdirectory without the file is
skipped with a warning, and one that fails to parse is skipped with an error
(not fatal). The loaded paths resolve the per-component **scripts** (paths
relative to the component directory):

| `build.*` field   | script               | invoked as                                     | produces                       |
| ----------------- | -------------------- | ---------------------------------------------- | ------------------------------ |
| `deps`            | `install_deps.sh`    | `<script> <repo>`                              | (side effects: dnf installs)   |
| `get-version`     | `get_version.sh`     | `<script>` (cwd=repo)                          | the long version on stdout     |
| `rpm.build`       | `build_rpms.sh`      | `<script> <repo> <el> <rpms-topdir> <version>` | RPMs under the topdir          |
| `rpm.release-rpm` | `get_release_rpm.sh` | `<script> <el>`                                | the release-RPM name on stdout |

`install_deps.sh`, `build_rpms.sh`, and `get_version.sh` run with cwd set to the
component worktree; `get_release_rpm.sh` runs with no cwd. A non-zero exit from
any script is a `BuilderError`. A missing script is handled per script:
`get_version.sh` absent → `MissingScriptError { script }`; `install_deps.sh` /
`build_rpms.sh` absent → `BuilderError`; `get_release_rpm.sh` absent → `None`
(the component is dropped from the release — see Stage 4).

## Orchestration — `Builder.run`

Source: `builder/builder.py`. The in-container flow, returning a
`BuildArtifactReport` (002):

1. **`prepare_builder()`** — install the build toolchain (below).
2. **Skopeo short-circuit** — `skopeo_image_exists(canonical_uri)` (008); if the
   image already exists, write a `skipped: true` report
   (`container_image.pushed = false`) and return early.
3. **Existing-release check** — if `storage.s3` is configured,
   `check_release_exists(version)` (005). A complete release for the arch is
   reused. **`--force` is fixed here**: in Python `force` only switches a log
   message — `_build_release` is gated solely by `if not release_desc`, so a
   pre-existing complete release makes `--force` a no-op (the real
   component-level force, which skips `check_released_components`, is reached
   only when no complete release exists). The port makes `--force` ignore the
   existing `release_desc` so `_build_release` actually runs and the
   component-level reuse is skipped. With no `storage.s3`, the check (and the
   upload) is skipped.
4. **`_build_release`** — `prepare_components` (stage 1) then
   `_do_build_release`:
   - `check_released_components` (005) finds component versions already in S3
     (skipped under `--force`); only the rest are built.
   - `_build` → `build_rpms` (stage 2) → `sign_rpms` (stage 3, if a GPG signing
     id is configured) → `_upload` (stage 4) → assemble per-arch
     `ReleaseBuildEntry` + per-component `ReleaseComponent`, then
     `release_desc_upload` + `release_upload_components` (005).
   - If `storage.s3` is unset, the build runs but nothing uploads and the
     release build returns `None`.
5. **Container image** —
   `ContainerBuilder(desc, release_desc, components).build().finish(secrets, sign_with_transit)`
   (008).
6. **Report** — assemble and write `build-report.json` to the scratch mount
   (below).

`skip_build` and `force` are `BuildOptions` fields threaded through. The arch is
`x86_64` and build type `rpm` today (the descriptor's `el_version` gives
`el<N>`); a FIXME in the Python notes arch should come from the descriptor — the
port keeps the single-arch behavior and records the same limitation.

## Stage 1 — prepare

Source: `builder/prepare.py`.

- **`prepare_builder()`** — `dnf update`, install `epel-release`, enable the
  `crb` repo, `dnf update`, then install the toolchain (`git`, `wget`,
  `rpm-build`, `rpmdevtools`, `gcc-c++`, `createrepo`, `rpm-sign`, `pinentry`,
  `s3cmd`, `jq`, `ccache`, `buildah`, `skopeo`), then install `cosign` from a
  pinned GitHub release RPM (tolerating "already installed", exit 2). Each step
  streams via the subprocess out-callback.
- **`prepare_components(...)`** — an async scope that, **per component in
  parallel**, clones (`secrets.git_url_for(repo)` → `git_clone`, 003/004),
  checks out the ref into a worktree (`git_checkout`), applies patches, and
  records
  `BuildComponentInfo { name, repo_path, worktree_path, repo_url, base_ref, sha1, long_version }`
  (`git_get_sha1` + `get_version.sh`). On scope exit (or any failure) it removes
  the worktrees. The fan-out uses `TaskGroup` → the port mirrors it with an
  abort-on-first-error `JoinSet` (fail-fast with cancellation, per 005).
- **Patch selection** (`_get_patch_list`) — patches live under the component's
  `patches/` directory; a numeric `NNNN-...patch` prefix orders them, and nested
  directories named for the exact / minor (`M.m.p`) / major (`M.m`) version
  select version-specific patch sets, applied deepest-priority-first. This
  selection logic is ported faithfully (it is intricate; a golden test pins the
  ordering).

## Stage 2 — rpmbuild

Source: `builder/rpmbuild.py`. `ComponentBuild { version, rpms_path }`.

- **`_install_deps`** runs each component's `install_deps.sh` **sequentially**
  (it mutates shared system state via `dnf`).
- **`build_rpms`** sets up an `rpmbuild` topdir per component
  (`<rpms>/<name>/<version>/{BUILD,SOURCES,RPMS,SRPMS,SPECS}`) and runs each
  `build_rpms.sh` **in parallel** (`TaskGroup` → `JoinSet`), passing
  `CES_CCACHE_PATH` when a ccache is configured. `skip_build` creates the topdir
  but does not run the script.
- **`reset_python_env` is dropped** (001/003): Python scrubbed the venv
  `python3` from `PATH` before each script so they found the system interpreter;
  the Rust `cbsbuild` runs from no venv, so the build/deps scripts inherit a
  clean `PATH` already and the workaround is moot.

## Stage 3 — signing

Source: `builder/signing.py`. Runs only when a GPG signing id is configured
(`config.signing.gpg`). `sign_rpms` resolves the keyring via
`secrets.gpg_signing_key(id)` (004, an RAII keyring guard), then signs each
component's RPMs **in parallel across components, sequentially within a
component** (the Python notes within-component parallelism as a future exercise;
the port keeps the sequential-within behavior).

`_sign_rpm` runs
`rpm --addsign --define "_gpg_path <keyring>" --define "_gpg_name <email>"` and,
when a passphrase is set, a
`--define "_gpg_sign_cmd_extra_args --pinentry-mode loopback --passphrase <passphrase>"`.
**Redaction**: the passphrase is embedded inside a `--define` value, so the port
wraps that argument as a `SecureArg` (003) so it is redacted in logs/errors; the
003 pattern backstop also catches `--passphrase`. Note the inherent exposure —
`rpm --addsign` with a loopback passphrase places it on the child's command line
(visible to `ps` inside the build container); this is Python's behavior too and
is out of scope to change here.

## Stage 4 — upload

Source: `builder/upload.py`. `s3_upload_rpms` uploads each component's RPMs **in
parallel** (`TaskGroup` → `JoinSet`) under the per-component base
`<bucket_loc>/<name>/rpm-<version>/el<el_version>.clyso/`. The `RPMS` and
`SRPMS` trees land **asymmetrically** (matching `upload.py:133-136`): `RPMS`
contents are flattened directly under the base (relative to the `RPMS` dir),
while `SRPMS` contents keep an `SRPMS/` prefix (relative to the topdir). So the
keys are `<base>/<rpm>`, `<base>/repodata/...`, `<base>/SRPMS/<srpm>`, and
`<base>/SRPMS/repodata/...`. `createrepo` generates the `repodata` where RPMs
exist; everything uploads via `s3_upload_files(public=true)` (005), returning an
`S3ComponentLocation { name, version, location }`.

The per-component release-RPM location is resolved by
`get_component_release_rpm` (runs `get_release_rpm.sh <el>`, **no cwd**;
`releases/utils.py`) and recorded in the `ReleaseRPMArtifacts` (002). **Silent
component drop (Python behavior, reproduced):** if a component has no
`get_release_rpm.sh` (or it yields nothing), `get_component_release_rpm` returns
`None` and the orchestrator **drops that component from the release descriptor**
(`builder.py:576-582`, with an error log) — even though its RPMs were already
built, signed, and uploaded. The port reproduces this with the same clear error
log and flags it as a known quirk to revisit (the RPMs are otherwise orphaned in
S3, unreferenced by the release).

## Build report

`Builder` assembles a `BuildArtifactReport` (002) — version, `skipped`,
`container_image`, `release_descriptor` (S3 path + bucket), and per- component
`ComponentReport`s — and writes it to `build-report.json` on the **scratch
mount** (it is written on both the skipped and full-build paths). The **host**
runner reads it back across the scratch mount before the return-code check, with
partial-on-failure semantics, and unlinks it — that host-side round-trip is
specified in 009 (correctness invariant 5).

## Concurrency

Every per-component fan-out here (`prepare_components`, `build_rpms`,
`sign_rpms`, `s3_upload_rpms`) uses `asyncio.TaskGroup` with `ExceptionGroup`
aggregation — **fail-fast with cancellation**: the first failure cancels the
siblings. The port mirrors this with a `JoinSet` that aborts on the first error
(per 005), **not** `join_all`. `_install_deps` is deliberately **sequential**,
and signing is **sequential within** a component — both preserved.

## Errors

Per 002, these IO-layer errors live with this subsystem (in `cbscore`):
`BuilderError` (any stage failure) and `MissingScriptError { script }` (a
required component script is absent). Stage helpers wrap lower-level errors
(`GitError`, `S3Error`, `SecretsMgrError`, `CommandError`) into `BuilderError`
with context.

## Fidelity notes

- **`reset_python_env` dropped** — no venv on the `cbsbuild` `PATH` (001/003),
  so build/deps scripts need no PATH scrub.
- **GPG passphrase** travels in a `--define` value to `rpm --addsign`; wrapped
  as a `SecureArg` for logs, but inherently on the child command line (as in
  Python).
- **cosign** is installed from a pinned GitHub release RPM in `prepare_builder`;
  the version pin is carried over.
- **Single arch** (`x86_64`) / `rpm` build type / `el<N>` from the descriptor —
  the Python's "arch should come from the descriptor" FIXME is carried as a
  known limitation, not fixed here.
- **`cbs.component.yaml` is unversioned** (a read input).
- **RPM S3 layout** `<loc>/<name>/rpm-<version>/el<el>.clyso/...` is preserved
  (on-disk/S3 layout parity, invariant 3).
- **Sequencing**: deps sequential; rpmbuild/sign/upload parallel across
  components; sign sequential within a component.
- **`--force` fixed** — Python's release-level `--force` is a no-op when a
  complete release exists (logs only); the port makes it actually rebuild
  (Orchestration step 3).
- **Silent component drop** — a component with no `get_release_rpm.sh` is
  dropped from the release descriptor though its RPMs are uploaded (Python
  behavior, reproduced + flagged as a quirk; Stage 4).
- **Patch traversal — stray files tolerated** (added in v2) — while walking
  `patches/`, the port's `collect_patches` (`prepare.rs`) **skips** any
  non-`.patch` plain file. Python's `_get_patches_by_prio` recurses into every
  non-`.patch` entry, but its depth>0 version-name guard prunes entries whose
  name isn't the exact/minor/major version — so common strays (`README`,
  `series`) are harmlessly skipped by both. The one real divergence: a stray
  non-`.patch` **file named exactly like a version selector** (e.g. a file
  literally named `1.2.3`) makes Python call `iterdir()` on a non-directory and
  raise `NotADirectoryError` (build failure), where the port skips it. A
  malformed `NNNN-` patch name is warned + skipped in both. Surfaced by the C3
  implementation review.
- **Dead code omitted** — the port does not copy Python's unused blocks (e.g.
  the trailing `patches_lst` computation in `_apply_patches`, built and never
  read).

## Testing

- **Component model**: `load_components` over a `components/` tree loads each
  `cbs.component.yaml` (kebab keys `get-version`/`release-rpm`), skips a dir
  with no manifest, and skips (not fails) a malformed one.
- **Patch ordering**: golden test of `_get_patch_list` over a nested `patches/`
  tree (numeric prefix order; version/minor/major dir selection; deepest-first).
- **Orchestration decision flow**: image-exists → skipped report; existing
  release + no `--force` → reuse; **`--force` → actual rebuild even when a
  complete release exists** (the port's fix of Python's no-op force); no
  `storage.s3` → build-but-no-upload returns no release.
- **Stage behavior**: `skip_build` makes the topdir without running the script;
  ccache sets `CES_CCACHE_PATH`; signing is skipped without a GPG id; the RPM S3
  keys match the layout above.
- **Redaction**: the GPG passphrase never appears in a captured log line.
- **Concurrency**: a single component failure aborts the fan-out with an
  aggregated `BuilderError` (fail-fast); `_install_deps` runs in order.
- **Report**: a `BuildArtifactReport` is written to the scratch
  `build-report.json` on both the skipped and full-build paths.
