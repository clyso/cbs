# 001 — Architecture & implementation spine: cbscore Rust port

This is the orientation document for the Rust port of the Python `cbscore` build
library. It fixes the crate layout, the cross-cutting conventions, and the build
target, and it owns the **capability commit map** that sequences the work. The
subsystem reference designs (002 and onward) describe each subsystem in depth;
the implementation plans (`001-NN`) are derived from the commit map here. Read
this doc first.

## Context, goals, non-goals

`cbscore` is the CES Build System core: a CLI (`cbsbuild`) that builds and
releases Ceph (and other) containers. It is a two-phase system — a host
orchestrator spawns a builder container from the descriptor's own image and runs
the actual build inside it.

The port exists so that `cbsd-worker` can drive a build by calling the runner
**in-process**, linking a Rust `cbscore` library, instead of shelling out to a
Python helper (`cbsd-rs/scripts/cbscore-wrapper.py`). In-process use lets the
worker consume the runner's `Result` and its output natively rather than
classifying subprocess exit codes and parsing stdout.

**Goals.** Faithful behavioral parity with Python `cbscore` (except where the
Python is itself broken — see the commit map's notes on `versions list`);
end-to-end static typing; a single async runtime (`tokio`); no embedded Python
interpreter for cbscore inside the builder container; direct in-process worker
integration.

**Non-goals.** Cross-language byte-equality of wire formats — round-trip
stability within Rust suffices. No steady-state file interchange between Python
and Rust cbscore: a deployment runs one or the other. **No `config init`
command** — see "Configuration is hand-authored" below.

## Crate layout

Three new members of the existing `cbsd-rs/` Cargo workspace.

- **`cbscore-types`** — zero-IO. Wire types (version/container/release/ image
  descriptors, config, build report), the error taxonomy (`thiserror`),
  `schema_version` markers, the `VersionType` enum and its type-label table, and
  tracing-target constants. Dependencies limited to `serde`, `serde_json`,
  `thiserror`, `camino`, `uuid`. **No** `tokio`, IO, cloud SDKs, or `regex`.
- **`cbscore`** — the library. Subprocess execution + secret redaction,
  shell-tool wrappers (`git`, `podman`, `buildah`, `skopeo`), config load/store,
  the secrets manager (file- and git-backed), the Vault client, the S3 client,
  releases, versions logic, the builder pipeline, containers, images, and the
  runner. May depend on `tokio`, `aws-sdk-s3`, `vaultrs`, `serde_saphyr`,
  `regex`, `rand`, `tempfile`, `which`.
- **`cbsbuild`** — the thin CLI binary. A `clap` tree mirroring the Python
  command set (minus `config`), tracing-subscriber setup, log-file routing, and
  the top-level error-to-exit-code mapping. **This is the artifact mounted into
  the builder container** (see Build target).

**Dependency direction:** `cbscore-types` ← `cbscore` ← `cbsbuild`, and the
existing `cbsd-worker` ← `cbscore`. No cycles; nothing depends on `cbsbuild`.

**Visibility.** Workspace-internal items are `pub(crate)`; `pub` is reserved for
the genuine cross-crate surface. Every public item carries a doc comment.

**Lift-out invariants.** `utils::git` and `utils::subprocess` are designed to be
movable to a future shared-primitives sister crate (its name must not collide
with the existing `cbsd-common` workspace member — these are distinct crates).
They import only primitives or generics (never cbscore-internal types), use
their own tracing targets (`cbscore::utils::git`, `cbscore::utils::subprocess`),
and depend only on `tokio`, `tracing`, `thiserror`, `regex`, `camino`, `which` —
no cloud SDK, no `serde_saphyr`.

## Build target & portability (resolves review B1)

> **Extended/refined by design 012.** The acceptance gate below ("runs as PID 1
> in the oldest supported `desc.distro` (rockylinux:9)") is refined by
> `012-…-static-musl-acceptance-and-distro-independence.md`: a static-musl
> binary has no libc linkage and is distro-independent, so the operative gate is
> the **link-time staticness** check (`ldd`/`file`), and the runtime smoke run
> uses a _representative_ EL image, not Rocky 9 as a canonical target. See 012
> for the governing acceptance criteria.

The runner spawns the builder container from `desc.distro` (e.g. `rockylinux:9`
— EL9, glibc 2.34) and runs the mounted `cbsbuild` as PID 1 inside it. The build
host (and `cbsd-worker`) is not that image, so a glibc-dynamic binary built on
the host cannot run there.

- **`cbsbuild` is built static-musl** (`x86_64-unknown-linux-musl`), so it has
  no glibc dependency. The existing `rust-builder` stage in
  `container/ContainerFile.cbsd-rs` is `FROM alpine:3.21` and runs
  `cargo build --release --workspace` with no `--target` flag; on Alpine the
  host triple is already musl, so binaries are static musl **implicitly**. An
  explicit `--target` is only needed if building off-Alpine.
- **Two concrete `ContainerFile.cbsd-rs` changes are required** and are part of
  the commit map: (1) `cbsbuild` must be a build output — the workspace must
  include the crate so `cargo build --workspace` produces it (C0); (2) the
  worker image must `COPY` the `cbsbuild` artifact to a known path,
  `/usr/local/bin/cbsbuild` (C8). Today the worker image copies only
  `cbsd-worker` (`ContainerFile.cbsd-rs:158`).
- The runner mounts **that explicit path** into the builder container at
  `/runner/cbsbuild` and runs it as PID 1 — never "self" (`cbsd-worker` is a
  different binary).
- **Crate-stack validation gate (commit C0):** `aws-sdk-s3` (with a musl-clean
  crypto provider) and `vaultrs` must compile, link statically, and run on the
  musl target. This is proven in CI before any subsystem work begins (see C0's
  note — the proof is a CI job, not a shipped dependency edge).
- **Acceptance gate:** the static `cbsbuild` runs as PID 1 in the oldest
  supported `desc.distro` (rockylinux:9). Static linking makes this trivial, but
  it is verified, not assumed. _(Refined by 012: the operative gate is the
  distro-independent link-time staticness check; the runtime run is a secondary
  smoke test on a representative EL image, not pinned to Rocky 9.)_

## Failure isolation & async model (resolves review B2)

One async runtime: `tokio`, multi-thread. After the worker cutover, cbscore's
host-side orchestration (component aggregation, secrets marshalling, podman
invocation, report parsing, cleanup) runs in-process inside `cbsd-worker`. The
orchestration is `async` throughout, which constrains the isolation mechanisms:

- **Panic isolation is the tokio task boundary, not `catch_unwind`.**
  `std::panic::catch_unwind` wraps a synchronous closure and does not catch a
  panic raised across an `.await`, so it cannot wrap the async orchestration.
  The build already runs in a spawned tokio task; the worker maps
  `JoinError::is_panic()` (from awaiting the `JoinHandle`) onto the same
  `BuildFinishedStatus::Failure` path the subprocess exit-code classification
  produces today (`cbsd-worker/src/build/executor.rs`, `classify_exit_code`).
- **The worker release profile must pin `panic = "unwind"` explicitly**, so no
  future profile edit can switch it to `abort` and silently void this invariant.
  (The workspace sets no `panic` value today, so `unwind` already applies by
  default; the point is to pin it, not to fix a flip.)
- **Build cancellation is an explicit signal, not future-drop.** Rust has no
  async `Drop`, so a dropped future cannot `.await` a `podman stop`.
  Cancellation uses a `tokio_util::sync::CancellationToken` (or equivalent
  select-on-shutdown) that the runner observes and, **before** returning, awaits
  `podman stop <ctr_name>` on the builder container. The runner already knows
  the container name and already issues `podman stop` on timeout, so only the
  trigger is new; the container name must be plumbed to the cancellation
  handler.

Detail lives in 009 (runner) and 011 (worker integration); the policy is fixed
here as an architectural invariant.

## Errors & logging

Per-subsystem `thiserror` enums; the shared taxonomy lives in `cbscore-types`.
`anyhow` appears only at `cbsbuild`'s `main` boundary, where library errors
collapse to an exit code and a stderr message. Tracing uses a target hierarchy
(`cbscore::utils::subprocess`, `cbscore::runner`, …). `CBS_DEBUG` selects the
effective level and is forwarded into the builder container **explicitly** as
`-e CBS_DEBUG=<1|0>` derived from the host's effective debug state (resolves
review H3).

## Wire-format & schema-versioning policy (resolves review M2-of-000)

Only the formats cbscore **produces or owns** carry a version marker: the
version descriptor, the release descriptor, the config, and the secrets file
each get a `schema_version` integer. The build report keeps its existing
`report_version` marker for now; converging all markers on `schema_version`
across producer and consumers (worker, server) is a roadmap item
(`cbsd-rs/docs/ROADMAP.md`). The externally-authored container and image
descriptors — owned by component/release repos and only read by cbscore — are
**not** versioned. The bump policy is **pragmatic**:

- **Additive, backward-compatible** changes (a new optional field that is
  defaulted on read and skipped-if-absent on write) do **not** bump.
- **Breaking** changes (rename, remove, retype, or any semantic shift in an
  existing field) **do** bump.

Per-format key casing (kebab-case for config/secrets, snake_case for
descriptors/releases), absent/unknown-version handling, and the secrets-file
`type:` discriminator are specified in 002. There is no migration tool and no
cross-language interchange in steady state.

## Configuration is hand-authored

The Python `config init` / `init-vault` commands are **not** ported. They buy
little — even Python's `--for-systemd-install` mode still prompts interactively
for storage and signing — and they carry a large prompt/flag-bypass surface. The
Rust port drops the entire `config` command group.

Operators (and the `cbsd-rs` deployment tooling) author the config, secrets, and
vault YAML files **by hand**. This makes thorough documentation a **required
deliverable**: the cbscore-rs `README.md` must document every file's format —
all fields, which are required vs optional, the secrets-file `type:`
discriminators, the Vault auth blocks, and a complete worked example for a
worker deployment. Design 004 specifies the formats; the README is the
operator-facing rendering of them. (Interactive config UX, if ever wanted, is a
separate future effort, not in this port.)

## Capability commit map

Work is sliced **by capability — what an operator or the worker can do — not by
layer.** Foundational code (types, the subprocess primitive, the tool wrappers,
the S3 client, buildah) lands in the commit of its **first consumer**, never as
a standalone layer. Every commit compiles, works, and is independently
revertable. The "lands here" column is the evidence that no commit ships dead
code.

### M0 — Bootstrap & de-risk

| #   | Capability delivered                                                 | Foundational code landing here                                                                        |
| --- | -------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| C0  | A static `cbsbuild` runs as PID 1 in `rockylinux:9` (prints version) | workspace + 3 crate skeletons; `cbsbuild` added to the workspace build; **musl linkage proven in CI** |

C0 is an explicit one-time **de-risk spike** for B1, not a feature commit. The
musl static-linkage proof for `aws-sdk-s3` + `vaultrs` lives in a CI job (a
build-matrix check), **not** as a shipped dependency edge: those crates are
added to `cbscore`'s manifest only in the commits whose code first uses them (S3
at C4/C6, Vault at C4), so C0 carries no dead dependency. C0 ships the workspace
skeleton and the "static binary runs in EL9" acceptance check.

### M1 — Versions create

| #   | Capability delivered                                                                | Foundational code landing here                                                                                                                                                                                                                                                                                                                                                             |
| --- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| C1  | `cbsbuild versions create [VERSION]` writes a descriptor (auto UUIDv7 when omitted) | version types, parse helpers (`get_version_type` = name→type lookup, `parse_component_refs`), `version_create_helper`, **subprocess primitive + redaction + git wrapper** (first consumer: `get_git_user` via `git config`, `get_git_repo_root` via `rev-parse --show-toplevel`), `get_image_desc` + `ImageDescriptor` type (the trailing "image descriptor missing?" check), versions CLI |

`versions create` is **config-independent** (it takes CLI flags + the local git
repo), so no config types land here.

### M2 — Build (decomposed by working increment)

| #   | Capability delivered                                                                                  | Foundational code landing here                                                                                                                                                                                                                                                                                   |
| --- | ----------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| C2  | `cbsbuild build <desc>` spins up the builder container end-to-end and returns a report (**keystone**) | **runner (host)** + **config load/store** (first config consumer) + **secrets-file load/store** (host marshals the secrets file into the container) + components aggregation + config rewrite + **podman wrapper** + **report round-trip** + `Builder` skeleton (`prepare_builder`) + `build`/`runner build` CLI |
| C3  | build compiles a component's RPMs                                                                     | `prepare_components` (clone/worktree/patches) + `rpmbuild` (component scripts)                                                                                                                                                                                                                                   |
| C4  | build signs RPMs and uploads them + release descriptors to S3                                         | **`SecretsMgr` resolution (file+git) + Vault client** (first vault consumer: GPG/transit/registry creds; AppRole→userpass→token, `ces-kv`) + GPG signing + `createrepo` + **S3 write path** + releases write                                                                                                     |
| C5  | build assembles and pushes the container image                                                        | `ContainerBuilder` (container.yaml, pre/packages/post) + **buildah wrapper** + registry push                                                                                                                                                                                                                     |
| C6  | full-parity build: skip-if-image-exists, reuse existing release, transit-sign                         | **skopeo wrapper** (image-exists) + **S3 read path** (`check_release_exists` object download) + `check_released_components` reuse + cosign transit                                                                                                                                                               |

### M3 — Versions list

| #   | Capability delivered                                              | Foundational code landing here                                                                            |
| --- | ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| C7  | `cbsbuild versions list --from <addr>` lists releases (**fixed**) | S3 **object-listing** read path (`s3_list`); reuses config + secrets + S3 object-download landed by C4/C6 |

The Python `versions list` is **broken**: `versions_list` calls
`list_releases(secrets, url)` with two arguments (`cmds/versions.py:129`) but
`list_releases` requires four — `(secrets, url, bucket, bucket_loc)`
(`releases/s3.py:237-239`) — so it raises `TypeError` at runtime. There is no
working behavior to port. C7 **fixes** it: `bucket`/`loc` come from
`config.storage.s3.releases`, and `--from` supplies the S3 host URL. C7 is
sequenced after the build milestone so it reuses the config/secrets/S3
infrastructure those commits land, rather than introducing it.

### M4 — Worker integration

| #   | Capability delivered                                                                                                                              | Foundational code landing here                                                                                                                                |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| C8  | `cbsd-worker` runs builds in-process via the library; `cbscore-wrapper.py` retired & removed from the image; `cbsbuild` added to the worker image | `JoinError::is_panic()` failure mapping, `CancellationToken`→`podman stop`, `BuildDescriptor`→`VersionDescriptor` mapping, `ContainerFile` COPY of `cbsbuild` |

### M5 — Deferred (designed now, built later)

| #   | Capability delivered                                                       |
| --- | -------------------------------------------------------------------------- |
| C9  | configurable version-descriptor location (`--versions-dir` / config field) |

**Map → plans.** Each milestone becomes a capability plan
(`001-02-versions-create`, `001-03…07-build`, `001-08-versions-list`,
`001-09-worker`). The plan pins each commit to ~400–800 authored lines and
presents that breakdown for approval before any code is written. C2 is expected
to split there along capability seams (container mechanism
→ +`prepare_builder`), and C5 likewise (container.yaml/repos parse → buildah
assembly); the splits must remain capability-based, never layer-based.

## Subsystem design index

Each subsystem gets one reference design, written interface-first, as the single
source of truth for that subsystem. Plans pull interfaces from these docs.

| Seq | Subsystem design                    | Python source ported                                               | First consumed by                                            |
| --- | ----------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------ |
| 002 | Wire types & schema                 | `cbscore-types` surface across all modules                         | all                                                          |
| 003 | Subprocess, redaction & shell tools | `utils/{__init__,git,podman,buildah}`, `images/skopeo`             | C1 (subprocess+git), C2 (podman), C5 (buildah), C6 (skopeo)  |
| 004 | Configuration, secrets & Vault      | `config.py`, `utils/secrets/`, `utils/vault.py` (no `config init`) | C2 (config + secrets file), C4 (`SecretsMgr` + Vault client) |
| 005 | Storage (S3) & releases             | `utils/s3.py`, `releases/`                                         | C4 (write), C6 (object read), C7 (listing)                   |
| 006 | Versions                            | `versions/`, `images/desc.py`                                      | C1, C7, C9                                                   |
| 007 | Builder pipeline                    | `builder/`                                                         | C2, C3, C4, C6                                               |
| 008 | Containers & images                 | `containers/`, `images/{skopeo,signing,sync}`                      | C5, C6                                                       |
| 009 | Runner & two-phase                  | `runner.py`                                                        | C2                                                           |
| 010 | CLI surface                         | `cmds/` (minus `config`), `__main__.py`                            | C1, C2, C7                                                   |
| 011 | Worker integration                  | `cbsd-rs/scripts/cbscore-wrapper.py`, `cbsd-worker/src/build/`     | C8                                                           |

The `config` command group is intentionally absent from 010; design 004
specifies only the file formats + load/store + secrets/Vault, and the
operator-facing initialization is documented in the cbscore-rs README.

## Correctness invariants

Cross-cutting properties that are easy to get wrong; each is tested.

1. **Wire round-trip stability** — serialize → parse → equal for every format.
2. **CLI parity** — the per-subcommand flag table (010) is authoritative;
   `--cbscore-path` and `-e/--cbs-entrypoint` are intentionally dropped (both
   name the source mount the binary mount removes), and the `config` group
   (hand-authored config) and the empty `advanced` group are intentionally
   dropped.
3. **On-disk layout parity** — version store and scratch layout match Python.
4. **Secret redaction** — `SecureArg`; an argument's `Debug` emits the redacted
   form (a port hardening, stricter than Python; not tied to a 000-review
   finding).
5. **Build-report round-trip** — written in-container to the scratch mount, read
   on the host **before** the return-code check (partial-report-on-failure),
   then unlinked (resolves review H1).
6. **Failure isolation** — the in-process build runs in a spawned tokio task; a
   panic is detected via `JoinError::is_panic()` and mapped to the build-failure
   path; the worker profile pins `panic = "unwind"` (resolves review B2).
7. **Binary portability** — the static-musl `cbsbuild` runs as PID 1 in the
   oldest supported `desc.distro` (resolves review B1). _(Refined by design 012:
   the gate is the distro-independent link-time staticness check; a static-musl
   binary is not pinned to any one distro.)_
8. **Vault auth order** — AppRole → userpass → token; KV v2 mount `ces-kv`
   (resolves review H5).
9. **S3 addressing** — credentials from the secrets store injected as a static
   provider; `endpoint_url` from the secrets hostname; path-style addressing
   validated for MinIO/RGW (resolves review H4).
10. **Documented hand-authored config** — the cbscore-rs README fully specifies
    the config/secrets/vault file formats; on-disk output must match what the
    formats in 004 define.
